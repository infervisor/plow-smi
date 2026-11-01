//! Intel Level Zero Sysman GPU metrics backend.
//!
//! Architecture allows replacing or extending with Intel Management Library /
//! XPUM later by adding `backend/intel_xpum.rs` and registering it in
//! [`crate::GpuManager`] without changing [`crate::GpuBackend`].

use crate::backend::CachedMetrics;
use crate::device::{GpuDevice, Vendor};
use crate::error::{GpuError, Result};
use crate::ffi::levelzero::{
    format_uuid, props_name, zes_string, LevelZeroApi, ZeDriverHandle, ZesDeviceHandle,
    ZesEngineStats, ZesPowerEnergyCounter,
};
use crate::metrics::GpuBackend;

struct IntelSlot {
    #[allow(dead_code)]
    driver: ZeDriverHandle,
    zes: Option<ZesDeviceHandle>,
    identity: GpuDevice,
    metrics: CachedMetrics,
    prev_energy: Option<ZesPowerEnergyCounter>,
    prev_engine: Option<ZesEngineStats>,
}

/// Intel backend backed by runtime-loaded Level Zero (`libze_loader`).
pub struct IntelBackend {
    api: LevelZeroApi,
    slots: Vec<IntelSlot>,
}

// SAFETY: LevelZeroApi is Send+Sync; handles are opaque driver pointers.
unsafe impl Send for IntelBackend {}
unsafe impl Sync for IntelBackend {}

impl IntelBackend {
    /// Try to load Level Zero, initialize, and enumerate GPU devices.
    pub fn try_load() -> Result<Self> {
        let api = LevelZeroApi::load()?;
        api.init()?;
        let devices = api.enumerate_gpu_devices()?;

        let mut slots = Vec::with_capacity(devices.len());
        for (index, (driver, device)) in devices.into_iter().enumerate() {
            let props = api.device_properties(device);
            let (mut model, uuid) = if let Some(ref p) = props {
                let name = props_name(p);
                let model = if name.is_empty() {
                    format!("Intel GPU {index}")
                } else {
                    name
                };
                (model, format_uuid(&p.uuid))
            } else {
                (format!("Intel GPU {index}"), format!("intel-gpu-{index}"))
            };

            let zes = props.as_ref().and_then(|p| api.sysman_device(&p.uuid));
            let mut identity = GpuDevice::new(Vendor::Intel, index, uuid, model.clone());

            if let Some(zes_h) = zes {
                if let Some(sp) = api.sysman_device_properties(zes_h) {
                    if let Some(m) =
                        zes_string(&sp.model_name).or_else(|| zes_string(&sp.brand_name))
                    {
                        model = m;
                        identity.model = model;
                    }
                    identity.serial = zes_string(&sp.serial_number);
                }
                identity.pci_bus_id = api.pci_bus_id(zes_h).unwrap_or_default();
            }

            slots.push(IntelSlot {
                driver,
                zes,
                identity,
                metrics: CachedMetrics::default(),
                prev_energy: None,
                prev_engine: None,
            });
        }

        if slots.is_empty() {
            return Err(GpuError::DeviceQueryFailed(
                "Level Zero loaded but no GPUs enumerated".into(),
            ));
        }

        let mut backend = Self { api, slots };
        backend.refresh();
        Ok(backend)
    }
}

impl GpuBackend for IntelBackend {
    fn vendor(&self) -> Vendor {
        Vendor::Intel
    }

    fn device_count(&self) -> usize {
        self.slots.len()
    }

    fn devices(&self) -> Vec<GpuDevice> {
        self.slots.iter().map(|s| s.identity.clone()).collect()
    }

    fn refresh(&mut self) {
        for slot in &mut self.slots {
            slot.metrics = CachedMetrics::default();
            let Some(zes) = slot.zes else {
                continue;
            };

            if let Some((used, total)) = self.api.memory_state(zes) {
                slot.metrics.memory_used = Some(used);
                slot.metrics.memory_total = Some(total);
                slot.identity.memory_total = Some(total);
            }
            slot.metrics.temperature = self.api.temperature_c(zes);
            if let Some(mhz) = self.api.frequency_mhz(zes) {
                slot.metrics.clock_graphics = Some(mhz);
            }
            slot.metrics.fan_speed = self.api.fan_speed_percent(zes);
            slot.metrics.clock_memory = None;

            // Power (W) from energy counter deltas between refreshes.
            if let Some(cur) = self.api.energy_counter(zes) {
                if let Some(prev) = slot.prev_energy {
                    let de = cur.energy.saturating_sub(prev.energy) as f64; // µJ
                    let dt = cur.timestamp.saturating_sub(prev.timestamp) as f64; // µs
                    if dt > 0.0 && cur.energy >= prev.energy {
                        // µJ / µs = W
                        slot.metrics.power_usage = Some((de / dt) as f32);
                    }
                }
                slot.prev_energy = Some(cur);
            }

            // Utilization (%) from engine active-time deltas.
            if let Some(cur) = self.api.engine_activity(zes) {
                if let Some(prev) = slot.prev_engine {
                    let da = cur.active_time.saturating_sub(prev.active_time) as f64;
                    let dt = cur.timestamp.saturating_sub(prev.timestamp) as f64;
                    if dt > 0.0 && cur.active_time >= prev.active_time {
                        let pct = (100.0 * da / dt).clamp(0.0, 100.0) as f32;
                        slot.metrics.utilization = Some(pct);
                    }
                }
                slot.prev_engine = Some(cur);
            }
        }
    }

    fn utilization(&self, id: usize) -> Option<f32> {
        self.slots.get(id).and_then(|s| s.metrics.utilization)
    }

    fn memory_used(&self, id: usize) -> Option<u64> {
        self.slots.get(id).and_then(|s| s.metrics.memory_used)
    }

    fn memory_total(&self, id: usize) -> Option<u64> {
        self.slots
            .get(id)
            .and_then(|s| s.metrics.memory_total.or(s.identity.memory_total))
    }

    fn temperature(&self, id: usize) -> Option<f32> {
        self.slots.get(id).and_then(|s| s.metrics.temperature)
    }

    fn power_usage(&self, id: usize) -> Option<f32> {
        self.slots.get(id).and_then(|s| s.metrics.power_usage)
    }

    fn fan_speed(&self, id: usize) -> Option<f32> {
        self.slots.get(id).and_then(|s| s.metrics.fan_speed)
    }

    fn clock_graphics(&self, id: usize) -> Option<u32> {
        self.slots.get(id).and_then(|s| s.metrics.clock_graphics)
    }

    fn clock_memory(&self, id: usize) -> Option<u32> {
        self.slots.get(id).and_then(|s| s.metrics.clock_memory)
    }
}

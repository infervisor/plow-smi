//! AMD SMI GPU metrics backend.

use crate::backend::CachedMetrics;
use crate::device::{GpuDevice, Vendor};
use crate::error::{GpuError, Result};
use crate::ffi::amdsmi::{AmdSmiApi, ProcessorHandle, AMDSMI_CLK_TYPE_DF, AMDSMI_CLK_TYPE_SYS};
use crate::metrics::GpuBackend;

struct AmdSlot {
    handle: ProcessorHandle,
    identity: GpuDevice,
    metrics: CachedMetrics,
}

/// AMD backend backed by runtime-loaded AMD SMI (`libamd_smi`).
pub struct AmdBackend {
    api: AmdSmiApi,
    slots: Vec<AmdSlot>,
}

// SAFETY: AmdSmiApi is Send+Sync; processor handles are opaque.
unsafe impl Send for AmdBackend {}
unsafe impl Sync for AmdBackend {}

impl AmdBackend {
    /// Try to load AMD SMI, initialize, and enumerate GPU processors.
    pub fn try_load() -> Result<Self> {
        let api = AmdSmiApi::load()?;
        api.init()?;
        let handles = api.enumerate_gpus()?;

        let mut slots = Vec::with_capacity(handles.len());
        for (index, handle) in handles.into_iter().enumerate() {
            let (model, serial) = api
                .board_info(handle)
                .unwrap_or_else(|| ("AMD GPU".into(), None));
            let uuid = {
                let u = api.uuid(handle);
                if u.is_empty() {
                    format!("amd-gpu-{index}")
                } else {
                    u
                }
            };
            let mut identity = GpuDevice::new(Vendor::Amd, index, uuid, model);
            identity.serial = serial;
            identity.pci_bus_id = api.bdf_id(handle).unwrap_or_default();
            identity.architecture = String::new();
            let (total, _) = api.memory_bytes(handle);
            identity.memory_total = total;
            slots.push(AmdSlot {
                handle,
                identity,
                metrics: CachedMetrics::default(),
            });
        }

        if slots.is_empty() {
            return Err(GpuError::DeviceQueryFailed(
                "AMD SMI loaded but no GPUs enumerated".into(),
            ));
        }

        let mut backend = Self { api, slots };
        backend.refresh();
        Ok(backend)
    }
}

impl GpuBackend for AmdBackend {
    fn vendor(&self) -> Vendor {
        Vendor::Amd
    }

    fn device_count(&self) -> usize {
        self.slots.len()
    }

    fn driver_version(&self) -> Option<String> {
        self.slots
            .first()
            .and_then(|s| self.api.driver_version(s.handle))
    }

    fn devices(&self) -> Vec<GpuDevice> {
        self.slots.iter().map(|s| s.identity.clone()).collect()
    }

    fn refresh(&mut self) {
        for slot in &mut self.slots {
            slot.metrics = CachedMetrics::default();
            let h = slot.handle;
            slot.metrics.utilization = self.api.gpu_busy_percent(h);
            let (total, used) = self.api.memory_bytes(h);
            slot.metrics.memory_total = total;
            slot.metrics.memory_used = used;
            if let (Some(t), Some(u)) = (total, used) {
                slot.identity.memory_total = Some(t);
                if t > 0 {
                    slot.metrics.memory_utilization = Some((100.0 * u as f32) / t as f32);
                }
            }
            slot.metrics.temperature = self.api.temperature_c(h);
            slot.metrics.power_usage = self.api.power_watts(h);
            slot.metrics.power_limit = self.api.power_limit_watts(h);
            slot.metrics.fan_speed = self.api.fan_rpm(h);
            slot.metrics.clock_graphics = self.api.clock_mhz(h, AMDSMI_CLK_TYPE_SYS);
            slot.metrics.clock_memory = self.api.clock_mhz(h, AMDSMI_CLK_TYPE_DF);
        }
    }

    fn utilization(&self, id: usize) -> Option<f32> {
        self.slots.get(id).and_then(|s| s.metrics.utilization)
    }

    fn memory_utilization(&self, id: usize) -> Option<f32> {
        self.slots
            .get(id)
            .and_then(|s| s.metrics.memory_utilization)
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

    fn power_limit(&self, id: usize) -> Option<f32> {
        self.slots.get(id).and_then(|s| s.metrics.power_limit)
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

impl AmdBackend {
    /// Whether the loaded driver exports a power-limit setter.
    pub fn supports_power_limit(&self) -> bool {
        self.api.supports_power_limit()
    }

    fn handle_at(&self, index: u32) -> Result<ProcessorHandle> {
        self.slots
            .get(index as usize)
            .map(|s| s.handle)
            .ok_or_else(|| GpuError::DeviceQueryFailed(format!("AMD GPU {index} not found")))
    }

    /// Current performance level (`0=auto`, `1=low`, `2=high`, `3=manual`).
    pub fn perf_level(&self, index: u32) -> Option<u32> {
        let h = self.handle_at(index).ok()?;
        self.api.perf_level(h)
    }

    /// Set performance level string: `auto` / `low` / `high` / `manual`.
    pub fn set_perf_level(&self, index: u32, level: &str) -> Result<()> {
        let h = self.handle_at(index)?;
        let code = match level.to_ascii_lowercase().as_str() {
            "auto" => 0u32,
            "low" => 1,
            "high" => 2,
            "manual" => 3,
            other => {
                return Err(GpuError::Unsupported(format!(
                    "unknown AMD perf level `{other}`"
                )));
            }
        };
        self.api.set_perf_level(h, code)
    }

    /// Current, default, min and max power limits.
    ///
    /// A zero from AMD SMI means "not reported" and becomes `None`, except
    /// for the minimum, where zero is a real value some boards report.
    pub fn power_limits(&self, index: u32) -> Result<crate::PowerLimits> {
        let h = self.handle_at(index)?;
        let nz = |uw: u64| (uw > 0).then_some(uw / 1000);
        Ok(match self.api.power_cap_info_uw(h) {
            Some((cur, def, min, max)) => crate::PowerLimits {
                current_mw: nz(cur),
                default_mw: nz(def),
                min_mw: (max > 0).then_some(min / 1000),
                max_mw: nz(max),
            },
            None => crate::PowerLimits {
                current_mw: self.api.power_limit_watts(h).map(|w| w as u64 * 1000),
                ..Default::default()
            },
        })
    }

    /// Set power limit in milliwatts.
    pub fn set_power_limit(&self, index: u32, milliwatts: u64) -> Result<()> {
        let h = self.handle_at(index)?;
        self.api.set_power_limit_mw(h, milliwatts)
    }
}

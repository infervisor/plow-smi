//! NVIDIA NVML GPU metrics backend.

use crate::backend::CachedMetrics;
use crate::device::{GpuDevice, Vendor};
use crate::error::Result;
use crate::ffi::nvml::{NvmlApi, NvmlDevice};
use crate::metrics::GpuBackend;
use crate::process::GpuProcessInfo;

struct NvidiaSlot {
    handle: NvmlDevice,
    identity: GpuDevice,
    metrics: CachedMetrics,
}

/// NVIDIA backend backed by runtime-loaded NVML.
pub struct NvidiaBackend {
    api: NvmlApi,
    slots: Vec<NvidiaSlot>,
}

// SAFETY: NvmlApi is Send+Sync; device handles are opaque NVML pointers.
unsafe impl Send for NvidiaBackend {}
unsafe impl Sync for NvidiaBackend {}

impl NvidiaBackend {
    /// Try to load NVML, initialize, and enumerate devices.
    pub fn try_load() -> Result<Self> {
        let api = NvmlApi::load()?;
        api.init()?;
        let count = api.device_count()?;
        let mut slots = Vec::with_capacity(count as usize);
        for index in 0..count {
            let handle = match api.device_handle(index) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let model = api
                .device_name(handle)
                .unwrap_or_else(|| format!("NVIDIA GPU {index}"));
            let uuid = api
                .device_uuid(handle)
                .unwrap_or_else(|| format!("nvidia-gpu-{index}"));
            let mut identity = GpuDevice::new(Vendor::Nvidia, index as usize, uuid, model);
            identity.pci_bus_id = api.device_pci_bus_id(handle).unwrap_or_default();
            identity.serial = api.device_serial(handle);
            identity.architecture = api.architecture(handle).unwrap_or_default();
            if let Some(mem) = api.memory_info(handle) {
                identity.memory_total = Some(mem.total);
            }
            slots.push(NvidiaSlot {
                handle,
                identity,
                metrics: CachedMetrics::default(),
            });
        }
        if slots.is_empty() {
            return Err(crate::error::GpuError::DeviceQueryFailed(
                "NVML loaded but no devices enumerated".into(),
            ));
        }
        let mut backend = Self { api, slots };
        backend.refresh();
        Ok(backend)
    }
}

impl GpuBackend for NvidiaBackend {
    fn vendor(&self) -> Vendor {
        Vendor::Nvidia
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
            let h = slot.handle;
            if let Some(util) = self.api.utilization(h) {
                slot.metrics.utilization = Some(util.gpu as f32);
                slot.metrics.memory_utilization = Some(util.memory as f32);
            }
            if let Some(mem) = self.api.memory_info(h) {
                slot.metrics.memory_used = Some(mem.used);
                slot.metrics.memory_free = Some(mem.free);
                slot.metrics.memory_total = Some(mem.total);
                slot.identity.memory_total = Some(mem.total);
            }
            if let Some(t) = self.api.temperature_c(h) {
                slot.metrics.temperature = Some(t as f32);
            }
            if let Some(mw) = self.api.power_usage_mw(h) {
                slot.metrics.power_usage = Some(mw as f32 / 1000.0);
            }
            if let Some(mw) = self.api.power_limit_mw(h) {
                slot.metrics.power_limit = Some(mw as f32 / 1000.0);
            }
            if let Some(fan) = self.api.fan_speed_percent(h) {
                slot.metrics.fan_speed = Some(fan as f32);
            }
            slot.metrics.clock_graphics = self.api.clock_graphics_mhz(h);
            slot.metrics.clock_memory = self.api.clock_memory_mhz(h);
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

    fn memory_free(&self, id: usize) -> Option<u64> {
        self.slots.get(id).and_then(|s| s.metrics.memory_free)
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

    fn driver_version(&self) -> Option<String> {
        self.api.driver_version()
    }

    fn cuda_version(&self) -> Option<String> {
        self.api.cuda_version()
    }

    fn collect_processes(&self) -> Vec<GpuProcessInfo> {
        let mut processes: Vec<GpuProcessInfo> = Vec::new();
        for (idx, slot) in self.slots.iter().enumerate() {
            for (pid, mem, kind) in self.api.running_processes(slot.handle) {
                if let Some(existing) = processes
                    .iter_mut()
                    .find(|p| p.pid == pid && p.gpu_index == idx as u32)
                {
                    existing.process_type = "C+G".into();
                    existing.gpu_memory_bytes = existing.gpu_memory_bytes.max(mem);
                } else {
                    processes.push(GpuProcessInfo {
                        pid,
                        gpu_index: idx as u32,
                        gpu_memory_bytes: mem,
                        process_type: kind.to_string(),
                    });
                }
            }
        }
        processes
    }
}

impl NvidiaBackend {
    /// Whether the loaded driver exports a power-limit setter.
    pub fn supports_power_limit(&self) -> bool {
        self.api.supports_power_limit()
    }

    fn device_at(&self, index: u32) -> Result<NvmlDevice> {
        self.slots
            .get(index as usize)
            .map(|s| s.handle)
            .ok_or_else(|| {
                crate::error::GpuError::DeviceQueryFailed(format!("GPU {index} not found"))
            })
    }

    /// Max graphics/memory clocks (MHz).
    pub fn max_clocks(&self, index: u32) -> Result<(u32, u32)> {
        let h = self.device_at(index)?;
        self.api
            .max_clocks_mhz(h)
            .ok_or_else(|| crate::error::GpuError::Unsupported("max clock info unavailable".into()))
    }

    /// Power limit constraints in milliwatts.
    pub fn power_limit_constraints(&self, index: u32) -> Result<(u32, u32)> {
        let h = self.device_at(index)?;
        self.api.power_limit_constraints_mw(h).ok_or_else(|| {
            crate::error::GpuError::Unsupported("power limit constraints unavailable".into())
        })
    }

    /// Current, default, min and max power limits.
    pub fn power_limits(&self, index: u32) -> Result<crate::PowerLimits> {
        let h = self.device_at(index)?;
        let (min, max) = self
            .api
            .power_limit_constraints_mw(h)
            .map_or((None, None), |(a, b)| (Some(a as u64), Some(b as u64)));
        Ok(crate::PowerLimits {
            current_mw: self.api.power_management_limit_mw(h).map(u64::from),
            default_mw: self.api.power_management_default_limit_mw(h).map(u64::from),
            min_mw: min,
            max_mw: max,
        })
    }

    /// Application clocks `(memory, graphics)` in MHz, when NVML reports them.
    pub fn applications_clocks(&self, index: u32) -> Option<(u32, u32)> {
        let h = self.device_at(index).ok()?;
        self.api.applications_clocks_mhz(h)
    }

    /// Persistence mode, when NVML reports it.
    pub fn persistence_mode(&self, index: u32) -> Option<bool> {
        let h = self.device_at(index).ok()?;
        self.api.persistence_mode(h)
    }

    /// Set power limit in milliwatts.
    pub fn set_power_limit(&self, index: u32, milliwatts: u32) -> Result<()> {
        let h = self.device_at(index)?;
        self.api.set_power_limit_mw(h, milliwatts)
    }

    /// Supported memory clocks (MHz).
    pub fn supported_memory_clocks(&self, index: u32) -> Result<Vec<u32>> {
        let h = self.device_at(index)?;
        self.api.supported_memory_clocks(h)
    }

    /// Supported graphics clocks for a memory clock (MHz).
    pub fn supported_graphics_clocks(&self, index: u32, mem_mhz: u32) -> Result<Vec<u32>> {
        let h = self.device_at(index)?;
        self.api.supported_graphics_clocks(h, mem_mhz)
    }

    /// Set application clocks (requires elevated privileges).
    pub fn set_applications_clocks(
        &self,
        index: u32,
        mem_mhz: u32,
        graphics_mhz: u32,
    ) -> Result<()> {
        let h = self.device_at(index)?;
        self.api.set_applications_clocks(h, mem_mhz, graphics_mhz)
    }

    /// Reset application clocks to defaults.
    pub fn reset_applications_clocks(&self, index: u32) -> Result<()> {
        let h = self.device_at(index)?;
        self.api.reset_applications_clocks(h)
    }

    /// PCIe generation and link width.
    pub fn pcie_info(&self, index: u32) -> Result<(Option<u32>, Option<u32>)> {
        let h = self.device_at(index)?;
        Ok(self.api.pcie_info(h))
    }
}

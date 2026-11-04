//! AMD GPU backend reading the `amdgpu` kernel driver directly (sysfs, hwmon,
//! KFD, DRM fdinfo). No ROCm library is loaded; see [`crate::amdgpu`].

use std::path::PathBuf;

use crate::amdgpu::{self, pci_ids, Device};
use crate::backend::CachedMetrics;
use crate::device::{GpuDevice, Vendor};
use crate::error::{GpuError, Result};
use crate::metrics::GpuBackend;
use crate::process::GpuProcessInfo;

struct AmdSlot {
    dev: Device,
    identity: GpuDevice,
    metrics: CachedMetrics,
}

/// AMD backend over the `amdgpu` kernel driver's sysfs ABI.
///
/// `fan_speed` is in RPM. `memory_utilization` is the memory controller busy
/// percentage (`mem_busy_percent`), matching NVML's meaning.
pub struct AmdBackend {
    root: PathBuf,
    slots: Vec<AmdSlot>,
    driver_version: Option<String>,
}

impl AmdBackend {
    /// Enumerate every GPU bound to `amdgpu` under the host root
    /// (`/`, or [`amdgpu::HOST_ROOT_ENV`] when set).
    pub fn try_load() -> Result<Self> {
        Self::try_load_at(amdgpu::host_root())
    }

    /// Enumerate GPUs under an explicit host root, e.g. `/host` in a container.
    pub fn try_load_at(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let mut names = pci_ids::Cache::default();
        let slots = amdgpu::discover(&root)?
            .into_iter()
            .enumerate()
            .map(|(index, dev)| {
                let mut identity =
                    GpuDevice::new(Vendor::Amd, index, dev.uuid(), dev.model(&root, &mut names));
                identity.pci_bus_id = dev.pci_addr.clone();
                identity.architecture = dev.gfx_target.clone().unwrap_or_default();
                identity.serial = dev.serial();
                AmdSlot {
                    dev,
                    identity,
                    metrics: CachedMetrics::default(),
                }
            })
            .collect();
        let mut backend = Self {
            driver_version: amdgpu::driver_version(&root),
            root,
            slots,
        };
        backend.refresh();
        Ok(backend)
    }

    fn dev_at(&self, index: u32) -> Result<&Device> {
        self.slots
            .get(index as usize)
            .map(|s| &s.dev)
            .ok_or_else(|| GpuError::DeviceQueryFailed(format!("AMD GPU {index} not found")))
    }

    /// Whether the driver exposes a writable power cap on any GPU.
    pub fn supports_power_limit(&self) -> bool {
        self.slots.iter().any(|s| s.dev.power_cap_writable())
    }

    /// Current performance level (`0=auto`, `1=low`, `2=high`, `3=manual`,
    /// `4..=8` profile/determinism modes), in AMD SMI's numbering.
    pub fn perf_level(&self, index: u32) -> Option<u32> {
        self.dev_at(index).ok()?.perf_level()
    }

    /// Set performance level: `auto` / `low` / `high` / `manual`. Requires root.
    pub fn set_perf_level(&self, index: u32, level: &str) -> Result<()> {
        let level = level.to_ascii_lowercase();
        if !amdgpu::PERF_LEVELS[..4].contains(&level.as_str()) {
            return Err(GpuError::Unsupported(format!(
                "unknown AMD perf level `{level}`"
            )));
        }
        self.dev_at(index)?.set_perf_level(&level)
    }

    /// Current, default, min and max power limits.
    pub fn power_limits(&self, index: u32) -> Result<crate::PowerLimits> {
        let caps = self.dev_at(index)?.power_caps_uw();
        let mw = |uw: Option<u64>| uw.map(|v| v / 1000);
        Ok(crate::PowerLimits {
            current_mw: mw(caps.current),
            default_mw: mw(caps.default),
            min_mw: caps.max.and(mw(caps.min)),
            max_mw: mw(caps.max),
        })
    }

    /// Set power limit in milliwatts. Requires root.
    pub fn set_power_limit(&self, index: u32, milliwatts: u64) -> Result<()> {
        self.dev_at(index)?
            .set_power_cap_uw(milliwatts.saturating_mul(1000))
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
        self.driver_version.clone()
    }

    fn devices(&self) -> Vec<GpuDevice> {
        self.slots.iter().map(|s| s.identity.clone()).collect()
    }

    fn refresh(&mut self) {
        for slot in &mut self.slots {
            let s = slot.dev.sample();
            if s.vram_total.is_some() {
                slot.identity.memory_total = s.vram_total;
            }
            slot.metrics = CachedMetrics {
                utilization: s.busy_percent,
                memory_utilization: s.mem_busy_percent,
                memory_used: s.vram_used,
                memory_total: s.vram_total,
                temperature: s.temperature_c,
                power_usage: s.power_w,
                power_limit: s.power_cap_w,
                fan_speed: s.fan_rpm,
                clock_graphics: s.sclk_mhz,
                clock_memory: s.mclk_mhz,
            };
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

    fn collect_processes(&self) -> Vec<GpuProcessInfo> {
        let devices: Vec<Device> = self.slots.iter().map(|s| s.dev.clone()).collect();
        amdgpu::procs::collect(&self.root, &devices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amdgpu::tests::FakeHost;

    #[test]
    fn backend_over_fake_host() {
        let host = FakeHost::new("backend");
        host.gpu("0000:03:00.0", 1, 53865, "2de8238c16749fb8");
        host.write("proc/sys/kernel/osrelease", "7.0.0-30-generic\n");
        let b = AmdBackend::try_load_at(&host.root).unwrap();
        assert_eq!(b.device_count(), 1);
        assert_eq!(b.driver_version().as_deref(), Some("7.0.0-30-generic"));
        let d = &b.devices()[0];
        assert_eq!(d.pci_bus_id, "0000:03:00.0");
        assert_eq!(d.architecture, "gfx1201");
        assert_eq!(d.memory_total, Some(34208743424));
        let m = b.snapshot(0).unwrap();
        assert_eq!(m.power_usage, Some(13.0));
        assert_eq!(m.memory_utilization, Some(7.0));
        assert_eq!(
            b.power_limits(0).unwrap(),
            crate::PowerLimits {
                current_mw: Some(300_000),
                default_mw: Some(300_000),
                min_mw: Some(210_000),
                max_mw: Some(300_000),
            }
        );
        b.set_power_limit(0, 250_000).unwrap();
        assert_eq!(b.power_limits(0).unwrap().current_mw, Some(250_000));
        assert!(b.set_perf_level(0, "profile_peak").is_err());
        b.set_perf_level(0, "HIGH").unwrap();
        assert_eq!(b.perf_level(0), Some(2));
        assert!(b.set_power_limit(5, 1).is_err());
    }
}

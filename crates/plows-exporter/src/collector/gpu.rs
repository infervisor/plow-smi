//! Unified GPU collector backed by `plows-gpu` (NVML / amdgpu sysfs / Level Zero).

use std::sync::Mutex;

use async_trait::async_trait;
use plows_gpu::{GpuManager, GpuProcessInfo, Vendor};
use tracing::debug;

use crate::collector::{Collector, GpuSnapshot};
use crate::error::{ExporterError, Result};

/// Which vendors to include when collecting snapshots.
#[derive(Debug, Clone, Copy)]
pub struct VendorFilter {
    /// Include NVIDIA devices.
    pub nvidia: bool,
    /// Include AMD devices.
    pub amd: bool,
    /// Include Intel devices.
    pub intel: bool,
    /// Include Apple Silicon devices.
    pub apple: bool,
}

impl VendorFilter {
    /// Enable every vendor.
    pub fn all() -> Self {
        Self {
            nvidia: true,
            amd: true,
            intel: true,
            apple: true,
        }
    }

    fn allows(self, vendor: Vendor) -> bool {
        match vendor {
            Vendor::Nvidia => self.nvidia,
            Vendor::Amd => self.amd,
            Vendor::Intel => self.intel,
            Vendor::Apple => self.apple,
            _ => false,
        }
    }

    /// Whether at least one vendor is enabled.
    pub fn any(self) -> bool {
        self.nvidia || self.amd || self.intel || self.apple
    }
}

/// Single collector that discovers all GPU backends via `plows-gpu`.
pub struct GpuCollector {
    filter: VendorFilter,
    manager: Option<Mutex<GpuManager>>,
    device_count: usize,
    hostname: String,
}

impl GpuCollector {
    /// Create an uninitialized collector with the given vendor filter.
    pub fn new(filter: VendorFilter) -> Self {
        Self {
            filter,
            manager: None,
            device_count: 0,
            hostname: whoami::fallible::hostname().unwrap_or_else(|_| "unknown".into()),
        }
    }

    /// Convenience: collect every vendor that `plows-gpu` can load.
    pub fn all_vendors() -> Self {
        Self::new(VendorFilter::all())
    }

    /// Device count after successful init.
    pub fn device_count(&self) -> u32 {
        self.device_count as u32
    }

    /// NVIDIA driver version when an NVIDIA backend is present.
    pub fn driver_version(&self) -> Option<String> {
        let mgr = self.manager.as_ref()?.lock().ok()?;
        mgr.driver_version()
    }

    /// CUDA version when NVIDIA exposes it.
    pub fn cuda_version(&self) -> Option<String> {
        let mgr = self.manager.as_ref()?.lock().ok()?;
        mgr.cuda_version()
    }

    /// GPU processes from backends that support process enumeration.
    pub fn collect_gpu_processes(&self) -> Vec<GpuProcessInfo> {
        let Some(manager) = &self.manager else {
            return Vec::new();
        };
        let Ok(mgr) = manager.lock() else {
            return Vec::new();
        };
        mgr.collect_processes()
    }

    /// Compact summary of loaded backends, e.g. `nvidia:1 amd:2`.
    pub fn backend_summary(&self) -> String {
        let Some(manager) = &self.manager else {
            return String::new();
        };
        let Ok(mgr) = manager.lock() else {
            return String::new();
        };
        mgr.backends()
            .iter()
            .filter(|b| self.vendor_allowed(b.vendor()))
            .map(|b| format!("{}:{}", b.vendor().as_str(), b.device_count()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn vendor_allowed(&self, vendor: Vendor) -> bool {
        self.filter.allows(vendor)
    }
}

#[async_trait]
impl Collector for GpuCollector {
    fn name(&self) -> &'static str {
        "gpu"
    }

    async fn init(&mut self) -> Result<usize> {
        if !self.filter.any() {
            return Err(ExporterError::Config(
                "no GPU vendors enabled in filter".into(),
            ));
        }

        let mgr = GpuManager::discover_filtered(|vendor| self.filter.allows(vendor));
        let count: usize = mgr
            .backends()
            .iter()
            .filter(|b| self.vendor_allowed(b.vendor()))
            .map(|b| b.device_count())
            .sum();

        if count == 0 {
            return Err(ExporterError::GpuInit(
                "no matching GPUs discovered via plows-gpu".into(),
            ));
        }

        self.device_count = count;
        self.manager = Some(Mutex::new(mgr));
        Ok(count)
    }

    async fn collect(&self) -> Result<Vec<GpuSnapshot>> {
        let manager = self
            .manager
            .as_ref()
            .ok_or_else(|| ExporterError::GpuInit("GPU collector not initialized".into()))?;

        let mut mgr = manager
            .lock()
            .map_err(|_| ExporterError::GpuInit("GPU manager lock poisoned".into()))?;

        mgr.refresh_all();

        let mut snapshots = Vec::with_capacity(self.device_count);
        let mut global_index = 0u32;
        for backend in mgr.backends() {
            if !self.vendor_allowed(backend.vendor()) {
                continue;
            }
            let vendor = backend.vendor().as_str();
            for (id, device) in backend.devices().into_iter().enumerate() {
                let Some(m) = backend.snapshot(id) else {
                    continue;
                };

                let mut snap = GpuSnapshot::new(
                    global_index,
                    vendor,
                    self.hostname.clone(),
                    device.model,
                    device.uuid,
                );
                global_index += 1;

                snap.gpu_utilization_percent = m.utilization.map(|v| v as i64);
                snap.memory_utilization_percent = m.memory_utilization.map(|v| v as i64);
                snap.memory_total_bytes = m.memory_total;
                snap.memory_used_bytes = m.memory_used;
                snap.memory_free_bytes = m.memory_free.or_else(|| {
                    let (total, used) = (m.memory_total?, m.memory_used?);
                    Some(total.saturating_sub(used))
                });
                snap.power_usage_mw = m.power_usage.map(|w| (w * 1000.0) as u64);
                snap.power_limit_mw = m.power_limit.map(|w| (w * 1000.0) as u64);
                snap.clock_core_mhz = m.clock_graphics;
                snap.clock_memory_mhz = m.clock_memory;
                snap.temperature_celsius = m.temperature.map(|t| t as i64);
                snap.fan_speed = m.fan_speed.map(|f| f as u32);
                snap.soc = m.soc;

                debug!(
                    index = snap.index,
                    vendor, "Collected GPU metrics via plows-gpu"
                );
                snapshots.push(snap);
            }
        }

        Ok(snapshots)
    }
}

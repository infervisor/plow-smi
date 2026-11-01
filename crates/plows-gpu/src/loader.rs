//! Runtime discovery of every available GPU backend.

use tracing::{info, warn};

use crate::backend::{AmdBackend, IntelBackend, NvidiaBackend};
use crate::device::GpuDevice;
use crate::error::GpuError;
use crate::metrics::GpuBackend;

/// Discovers and owns all GPU backends that successfully loaded on this host.
///
/// Multiple vendors may be active at once (e.g. NVIDIA + AMD). Missing
/// libraries are logged and skipped — never panicked on.
pub struct GpuManager {
    backends: Vec<Box<dyn GpuBackend>>,
}

impl GpuManager {
    /// Probe NVIDIA → AMD → Intel independently and keep every success.
    pub fn discover() -> Self {
        Self::discover_filtered(|_| true)
    }

    /// Probe only selected vendors, avoiding unused backend sampler threads.
    pub fn discover_filtered(allows: impl Fn(crate::Vendor) -> bool) -> Self {
        let mut backends: Vec<Box<dyn GpuBackend>> = Vec::new();

        if allows(crate::Vendor::Nvidia) {
            match NvidiaBackend::try_load() {
                Ok(b) => {
                    info!(devices = b.device_count(), "Loaded NVIDIA backend");
                    backends.push(Box::new(b));
                }
                Err(GpuError::LibraryNotFound { .. }) => {
                    info!("Missing NVML");
                }
                Err(e) => {
                    warn!(error = %e, "NVIDIA backend unavailable");
                }
            }
        }

        if allows(crate::Vendor::Amd) {
            match AmdBackend::try_load() {
                Ok(b) => {
                    info!(devices = b.device_count(), "Loaded AMD backend");
                    backends.push(Box::new(b));
                }
                Err(GpuError::LibraryNotFound { .. }) => {
                    info!("Missing AMD SMI");
                }
                Err(e) => {
                    warn!(error = %e, "AMD backend unavailable");
                }
            }
        }

        if allows(crate::Vendor::Intel) {
            match IntelBackend::try_load() {
                Ok(b) => {
                    info!(devices = b.device_count(), "Loaded Intel backend");
                    backends.push(Box::new(b));
                }
                Err(GpuError::LibraryNotFound { .. }) => {
                    info!("Missing Level Zero");
                }
                Err(e) => {
                    warn!(error = %e, "Intel backend unavailable");
                }
            }
        }

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if allows(crate::Vendor::Apple) {
            match crate::AppleBackend::try_load() {
                Ok(b) => {
                    info!(devices = b.device_count(), "Loaded Apple Silicon backend");
                    backends.push(Box::new(b));
                }
                Err(e) => warn!(error = %e, "Apple Silicon backend unavailable"),
            }
        }

        if backends.is_empty() {
            info!("No GPU backend available");
        }

        Self { backends }
    }

    /// Build a manager from pre-constructed backends (tests / custom probes).
    pub fn from_backends(backends: Vec<Box<dyn GpuBackend>>) -> Self {
        Self { backends }
    }

    /// Immutable view of active backends.
    pub fn backends(&self) -> &[Box<dyn GpuBackend>] {
        &self.backends
    }

    /// Mutable view of active backends.
    pub fn backends_mut(&mut self) -> &mut [Box<dyn GpuBackend>] {
        &mut self.backends
    }

    /// Number of successfully loaded backends.
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Flattened device list across all backends (vendor order of discovery).
    pub fn all_devices(&self) -> Vec<GpuDevice> {
        let mut out = Vec::new();
        for b in &self.backends {
            out.extend(b.devices());
        }
        out
    }

    /// Refresh live metrics on every backend.
    pub fn refresh_all(&mut self) {
        for b in &mut self.backends {
            b.refresh();
        }
    }

    /// Collect GPU processes across all backends that support it.
    pub fn collect_processes(&self) -> Vec<crate::process::GpuProcessInfo> {
        let mut out = Vec::new();
        let mut offset = 0u32;
        for b in &self.backends {
            out.extend(b.collect_processes().into_iter().map(|mut p| {
                p.gpu_index += offset;
                p
            }));
            offset += b.device_count() as u32;
        }
        out
    }

    /// First available NVIDIA driver version, if any NVIDIA backend loaded.
    pub fn driver_version(&self) -> Option<String> {
        self.backends.iter().find_map(|b| b.driver_version())
    }

    /// First available CUDA version string.
    pub fn cuda_version(&self) -> Option<String> {
        self.backends.iter().find_map(|b| b.cuda_version())
    }

    /// True when at least one backend loaded.
    pub fn has_gpu(&self) -> bool {
        !self.backends.is_empty()
    }
}

impl Default for GpuManager {
    fn default() -> Self {
        Self::discover()
    }
}

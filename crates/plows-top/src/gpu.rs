//! GPU data collection — multi-vendor via plows-exporter's plows-gpu collector.

use plows_exporter::collector::gpu::{GpuCollector as ExporterGpuCollector, VendorFilter};
use plows_exporter::collector::{Collector, GpuSnapshot};
use plows_gpu::GpuProcessInfo;

/// GPU collector wrapping the exporter's unified `plows-gpu` collector.
pub struct GpuCollector {
    inner: ExporterGpuCollector,
    pub device_count: u32,
    /// Human-readable backend mix, e.g. `nvidia:1 amd:2`.
    pub backend_summary: String,
    pub driver_version: String,
    pub cuda_version: String,
    initialized: bool,
    runtime: tokio::runtime::Runtime,
}

impl GpuCollector {
    /// Initialize GPU collection for every vendor `plows-gpu` can discover.
    pub fn new() -> Self {
        let mut inner = ExporterGpuCollector::new(VendorFilter::all());

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        let (initialized, device_count) = match rt.block_on(inner.init()) {
            Ok(count) => (true, count as u32),
            Err(_) => (false, 0),
        };

        let (backend_summary, driver_version, cuda_version) = if initialized {
            (
                inner.backend_summary(),
                inner.driver_version().unwrap_or_else(|| "N/A".into()),
                inner.cuda_version().unwrap_or_else(|| "N/A".into()),
            )
        } else {
            (String::new(), "N/A".into(), "N/A".into())
        };

        Self {
            inner,
            device_count,
            backend_summary,
            driver_version,
            cuda_version,
            initialized,
            runtime: rt,
        }
    }

    /// Collect current GPU metrics (NVIDIA + AMD + Intel as available).
    pub fn collect(&self) -> Vec<GpuSnapshot> {
        if !self.initialized {
            return Vec::new();
        }

        self.runtime
            .block_on(self.inner.collect())
            .unwrap_or_default()
    }

    /// Collect GPU processes (NVML when available).
    pub fn collect_processes(&self) -> Vec<GpuProcessInfo> {
        if !self.initialized {
            return Vec::new();
        }
        self.inner.collect_gpu_processes()
    }
}

impl Default for GpuCollector {
    fn default() -> Self {
        Self::new()
    }
}

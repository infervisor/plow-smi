//! Vendor backend implementations of [`crate::GpuBackend`].

mod amd;
pub(crate) mod apple;
mod intel;
mod nvidia;

pub use amd::AmdBackend;
pub use apple::AppleBackend;
pub use intel::IntelBackend;
pub use nvidia::NvidiaBackend;

/// Cached live metrics for one device index.
#[derive(Debug, Clone, Default)]
pub(crate) struct CachedMetrics {
    pub utilization: Option<f32>,
    pub memory_utilization: Option<f32>,
    pub memory_used: Option<u64>,
    pub memory_free: Option<u64>,
    pub memory_total: Option<u64>,
    pub temperature: Option<f32>,
    pub power_usage: Option<f32>,
    pub power_limit: Option<f32>,
    pub fan_speed: Option<f32>,
    pub clock_graphics: Option<u32>,
    pub clock_memory: Option<u32>,
}

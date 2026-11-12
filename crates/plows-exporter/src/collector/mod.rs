//! Collector trait and shared types for GPU/system metric collection.

pub mod manager;
pub mod network;
pub mod wireguard;

#[cfg(feature = "probe")]
pub mod probe;

#[cfg(feature = "gpu")]
pub mod gpu;

#[cfg(feature = "system")]
pub mod system;

#[cfg(feature = "tpu")]
pub mod tpu;

use async_trait::async_trait;

use crate::error::Result;

/// A snapshot of metrics for a single GPU device.
#[derive(Debug, Clone)]
pub struct GpuSnapshot {
    /// Device index (0-based).
    pub index: u32,
    /// Vendor identifier (`"nvidia"`, `"amd"`, or `"intel"`).
    pub vendor: &'static str,
    /// Hostname of this machine.
    pub hostname: String,
    /// GPU model/brand name (e.g., "NVIDIA RTX 4090", "AMD Instinct MI210").
    pub brand: String,
    /// Unique device identifier (UUID).
    pub uuid: String,
    /// GPU core utilization percentage (0–100).
    pub gpu_utilization_percent: Option<i64>,
    /// Memory controller utilization percentage (0–100).
    pub memory_utilization_percent: Option<i64>,
    /// Total GPU memory in bytes.
    pub memory_total_bytes: Option<u64>,
    /// Used GPU memory in bytes.
    pub memory_used_bytes: Option<u64>,
    /// Free GPU memory in bytes.
    pub memory_free_bytes: Option<u64>,
    /// Current power draw in milliwatts.
    pub power_usage_mw: Option<u64>,
    /// Power limit in milliwatts.
    pub power_limit_mw: Option<u64>,
    /// Core/graphics clock speed in MHz.
    pub clock_core_mhz: Option<u32>,
    /// Memory clock speed in MHz.
    pub clock_memory_mhz: Option<u32>,
    /// GPU temperature in degrees Celsius.
    pub temperature_celsius: Option<i64>,
    /// Fan speed (percentage or RPM depending on vendor).
    pub fan_speed: Option<u32>,
    /// Optional CPU/NPU/shared-memory telemetry; never treated as dedicated VRAM.
    pub soc: Option<plows_gpu::SocMetrics>,
}

impl GpuSnapshot {
    /// Create a new snapshot with only identity fields populated.
    pub fn new(
        index: u32,
        vendor: &'static str,
        hostname: String,
        brand: String,
        uuid: String,
    ) -> Self {
        Self {
            index,
            vendor,
            hostname,
            brand,
            uuid,
            gpu_utilization_percent: None,
            memory_utilization_percent: None,
            memory_total_bytes: None,
            memory_used_bytes: None,
            memory_free_bytes: None,
            power_usage_mw: None,
            power_limit_mw: None,
            clock_core_mhz: None,
            clock_memory_mhz: None,
            temperature_celsius: None,
            fan_speed: None,
            soc: None,
        }
    }
}

/// Trait that all metric collectors must implement.
#[async_trait]
pub trait Collector: Send + Sync {
    /// Human-readable name of this collector (e.g., "gpu", "system").
    fn name(&self) -> &'static str;

    /// Initialize the collector, detecting available devices.
    /// Returns the number of devices found.
    async fn init(&mut self) -> Result<usize>;

    /// Collect a snapshot of metrics from all detected devices.
    async fn collect(&self) -> Result<Vec<GpuSnapshot>>;
}

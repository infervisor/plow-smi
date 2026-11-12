//! Prometheus metric definitions and registry management.

pub mod family;
pub mod gpu_metrics;
pub mod network_metrics;
#[cfg(feature = "probe")]
pub mod probe_metrics;
pub mod registry;
pub mod soc_metrics;
pub mod system_metrics;
pub mod wireguard_metrics;

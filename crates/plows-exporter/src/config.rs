//! CLI configuration and application settings.

use clap::Parser;

/// GPU Metrics Exporter — Prometheus exporter for AMD, NVIDIA, Intel, and system metrics.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "plows-exporter",
    version,
    about = "Professional GPU metrics exporter for Prometheus",
    long_about = "Collects GPU and system metrics via plows-gpu (NVML / amdgpu sysfs / Level Zero) \
                  and system sources, exposing them as Prometheus metrics over HTTP."
)]
pub struct Config {
    /// Port for the Prometheus metrics HTTP endpoint.
    #[arg(short, long, default_value_t = 9835)]
    pub port: u16,

    /// Metrics collection interval in seconds.
    #[arg(short, long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..))]
    pub interval: u64,

    /// Enable NVIDIA GPU metrics collection.
    #[arg(long)]
    pub nvidia: bool,

    /// Enable AMD GPU metrics collection.
    #[arg(long)]
    pub amd: bool,

    /// Enable Intel GPU metrics collection.
    #[arg(long)]
    pub intel: bool,

    /// Enable Apple Silicon GPU, CPU and Neural Engine telemetry.
    #[arg(long)]
    pub apple: bool,

    /// Enable system metrics collection (CPU, memory, disk, network).
    #[arg(long)]
    pub system: bool,

    /// Enable Google Cloud TPU metrics collection.
    #[arg(long)]
    pub tpu: bool,

    /// Enable all collectors (equivalent to --nvidia --amd --intel --apple --system --tpu).
    #[arg(long)]
    pub all: bool,

    /// Bind address for the HTTP server.
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: String,

    /// Log level filter (e.g., info, debug, trace, warn, error).
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

impl Config {
    /// Returns true if NVIDIA collection is enabled (either explicitly or via --all).
    pub fn nvidia_enabled(&self) -> bool {
        self.nvidia || self.all
    }

    /// Returns true if AMD collection is enabled (either explicitly or via --all).
    pub fn amd_enabled(&self) -> bool {
        self.amd || self.all
    }

    /// Returns true if Intel collection is enabled (either explicitly or via --all).
    pub fn intel_enabled(&self) -> bool {
        self.intel || self.all
    }

    /// Returns true if any GPU vendor flag is enabled.
    pub fn gpu_enabled(&self) -> bool {
        self.nvidia_enabled() || self.amd_enabled() || self.intel_enabled() || self.apple_enabled()
    }

    /// Returns true if Apple Silicon collection is enabled.
    pub fn apple_enabled(&self) -> bool {
        self.apple || self.all
    }

    /// Returns true if system collection is enabled (either explicitly or via --all).
    pub fn system_enabled(&self) -> bool {
        self.system || self.all
    }

    /// Returns true if TPU collection is enabled (either explicitly or via --all).
    pub fn tpu_enabled(&self) -> bool {
        self.tpu || self.all
    }

    /// Returns true if no collectors are explicitly enabled.
    pub fn no_collectors_enabled(&self) -> bool {
        !self.gpu_enabled() && !self.system_enabled() && !self.tpu_enabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn apple_flag_and_all_enable_gpu_collection() {
        let apple = Config::parse_from(["plows-exporter", "--apple"]);
        assert!(apple.apple_enabled() && apple.gpu_enabled());
        assert!(!apple.nvidia_enabled() && !apple.no_collectors_enabled());
        let all = Config::parse_from(["plows-exporter", "--all"]);
        assert!(all.apple_enabled() && all.intel_enabled());
    }
}

#[cfg(test)]
mod interval_tests {
    use super::*;
    #[test]
    fn rejects_zero_interval() {
        assert!(Config::try_parse_from(["export", "--interval", "0"]).is_err());
    }
}

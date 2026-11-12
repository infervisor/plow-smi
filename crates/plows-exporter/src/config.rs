//! CLI configuration and application settings.

use std::net::Ipv4Addr;

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

    /// Enable network interface counters: bytes, packets, errors, drops, link state (Linux).
    #[arg(long)]
    pub network: bool,

    /// Enable WireGuard interface and peer metrics (runs `wg show all dump`; needs CAP_NET_ADMIN).
    #[arg(long)]
    pub wireguard: bool,

    /// Ping this IPv4 address every probe interval and export loss and round-trip time. Repeatable.
    #[arg(long = "probe", value_name = "IPV4")]
    pub probe: Vec<Ipv4Addr>,

    /// Also ping every WireGuard peer at the first /32 address in its allowed IPs. Implies --wireguard.
    #[arg(long)]
    pub probe_wireguard_peers: bool,

    /// Milliseconds between probe rounds.
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(200..=60_000))]
    pub probe_interval_ms: u64,

    /// Also report interfaces skipped by default (lo, veth*, docker*, br-*…). Glob; repeatable.
    #[arg(long, value_name = "GLOB")]
    pub network_include: Vec<String>,

    /// Enable all collectors (--nvidia --amd --intel --apple --system --tpu --network --wireguard).
    /// Probes send traffic, so they run only when asked for with --probe or --probe-wireguard-peers.
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

    /// Returns true if network interface collection is enabled (either explicitly or via --all).
    pub fn network_enabled(&self) -> bool {
        self.network || self.all
    }

    /// Returns true if WireGuard collection is enabled (explicitly, via --all, or for peer probes).
    pub fn wireguard_enabled(&self) -> bool {
        self.wireguard || self.all || self.probe_wireguard_peers
    }

    /// Returns true if any ICMP probe is configured.
    pub fn probe_enabled(&self) -> bool {
        !self.probe.is_empty() || self.probe_wireguard_peers
    }

    /// Returns true if no collectors are explicitly enabled.
    pub fn no_collectors_enabled(&self) -> bool {
        !self.gpu_enabled()
            && !self.system_enabled()
            && !self.tpu_enabled()
            && !self.network_enabled()
            && !self.wireguard_enabled()
            && !self.probe_enabled()
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
mod network_tests {
    use super::*;

    #[test]
    fn all_enables_network_and_wireguard_but_not_probes() {
        let all = Config::parse_from(["plows-exporter", "--all"]);
        assert!(all.network_enabled() && all.wireguard_enabled());
        assert!(!all.probe_enabled());
    }

    #[test]
    fn probes_parse_and_peer_probes_imply_wireguard() {
        let c = Config::parse_from([
            "plows-exporter",
            "--probe",
            "10.100.0.1",
            "--probe",
            "100.64.0.1",
            "--probe-wireguard-peers",
        ]);
        assert_eq!(c.probe.len(), 2);
        assert!(c.probe_enabled() && c.wireguard_enabled());
        assert!(!c.network_enabled() && !c.no_collectors_enabled());
        assert_eq!(c.probe_interval_ms, 1000);
    }

    #[test]
    fn bad_probe_settings_are_refused() {
        assert!(Config::try_parse_from(["e", "--probe", "fd00::1"]).is_err());
        assert!(Config::try_parse_from(["e", "--probe", "not-an-ip"]).is_err());
        assert!(Config::try_parse_from(["e", "--probe-interval-ms", "10"]).is_err());
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

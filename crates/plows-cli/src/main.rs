//! Plow SMI — Unified GPU management CLI.
//!
//! Single binary providing:
//!   plow-smi export   — Start Prometheus metrics exporter
//!   plow-smi top      — Interactive TUI GPU monitor
//!   plow-smi ctl      — GPU power & clock control (NVIDIA + AMD)

mod cmd_ctl;
mod cmd_export;

use clap::{Parser, Subcommand};
use plows_ctl::{OutputFormat, PerfLevel};

#[derive(Parser)]
#[command(
    name = "plow-smi",
    version,
    about = "Plow SMI — Professional GPU observability & control toolkit",
    long_about = "Unified binary for GPU monitoring, metrics export, and power control.\n\n\
                  Subcommands:\n\
                  • export  — Start Prometheus metrics HTTP server\n\
                  • top     — Interactive terminal GPU dashboard\n\
                  • ctl     — GPU power/clock control operations (NVIDIA + AMD)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Prometheus metrics exporter server
    Export(plows_exporter::config::Config),

    /// Interactive terminal GPU monitor (htop for GPUs)
    Top,

    /// GPU power & clock control
    #[command(subcommand)]
    Ctl(CtlCommands),
}

// ─── Ctl command structure (vendor-based) ────────────────────────────────────

#[derive(Subcommand)]
pub enum CtlCommands {
    /// Read control capabilities and resolved power profiles
    Capabilities {
        #[arg(long, default_value = "json")]
        format: OutputFormat,
    },
    /// Apply a profile or settings from JSON on stdin
    Apply {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "json")]
        format: OutputFormat,
    },
    /// CPU frequency controls
    #[command(subcommand)]
    Cpu(plows_ctl::cli::CpuCommands),
    /// List Apple Silicon devices (read-only)
    AppleList {
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
    /// Show Apple Silicon GPU/CPU/NPU telemetry (read-only)
    AppleInfo {
        #[arg(default_value_t = 0)]
        gpu: u32,
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
    /// List every GPU (NVIDIA + AMD + Intel)
    #[command(name = "list")]
    List {
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    // ── NVIDIA ──────────────────────────────────────────────────────────────
    /// List all detected NVIDIA GPUs
    #[command(name = "nvidia-list")]
    NvidiaList {
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show detailed info for a specific NVIDIA GPU
    #[command(name = "nvidia-info")]
    NvidiaInfo {
        /// GPU index (0-based)
        gpu: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Set NVIDIA application clock speeds
    #[command(name = "nvidia-set-clocks")]
    NvidiaSetClocks {
        /// GPU index (0-based), or use --all
        gpu: Option<u32>,
        /// Target memory clock (MHz)
        #[arg(long)]
        mem: u32,
        /// Target graphics/core clock (MHz)
        #[arg(long)]
        graphics: u32,
        /// Apply to all GPUs
        #[arg(long)]
        all: bool,
    },

    /// Set NVIDIA power limit (watts)
    #[command(name = "nvidia-set-power-limit")]
    NvidiaSetPowerLimit {
        /// GPU index (0-based)
        gpu: u32,
        /// Power limit in watts
        #[arg(long)]
        watts: u32,
    },

    /// Set NVIDIA performance level
    #[command(name = "nvidia-set-perf")]
    NvidiaSetPerf {
        /// GPU index (0-based), or use --all
        gpu: Option<u32>,
        /// Performance level: auto, low, high
        #[arg(long)]
        level: PerfLevel,
        /// Apply to all GPUs
        #[arg(long)]
        all: bool,
    },

    /// Reset NVIDIA clocks to default
    #[command(name = "nvidia-reset")]
    NvidiaReset {
        /// GPU index (0-based)
        gpu: Option<u32>,
        /// Reset all GPUs
        #[arg(long)]
        all: bool,
    },

    /// Show NVIDIA supported clock speeds for a GPU
    #[command(name = "nvidia-supported-clocks")]
    NvidiaSupportedClocks {
        /// GPU index (0-based)
        gpu: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    // ── AMD ─────────────────────────────────────────────────────────────────
    /// List all detected AMD GPUs
    #[command(name = "amd-list")]
    AmdList {
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show detailed info for a specific AMD GPU
    #[command(name = "amd-info")]
    AmdInfo {
        /// GPU index (0-based)
        gpu: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Set AMD power limit (watts)
    #[command(name = "amd-set-power-limit")]
    AmdSetPowerLimit {
        /// GPU index (0-based)
        gpu: u32,
        /// Power limit in watts
        #[arg(long)]
        watts: u32,
    },

    /// Set AMD performance level
    #[command(name = "amd-set-perf")]
    AmdSetPerf {
        /// GPU index (0-based), or use --all
        gpu: Option<u32>,
        /// Performance level: auto, low, high
        #[arg(long)]
        level: PerfLevel,
        /// Apply to all GPUs
        #[arg(long)]
        all: bool,
    },

    /// Reset AMD GPU to default settings
    #[command(name = "amd-reset")]
    AmdReset {
        /// GPU index (0-based)
        gpu: Option<u32>,
        /// Reset all GPUs
        #[arg(long)]
        all: bool,
    },

    /// List all detected Intel GPUs
    #[command(name = "intel-list")]
    IntelList {
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show detailed info for a specific Intel GPU
    #[command(name = "intel-info")]
    IntelInfo {
        gpu: u32,
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let log_level = match &cli.command {
        Commands::Export(config) => config.log_level.as_str(),
        _ => "warn",
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| log_level.into()),
        )
        .init();

    match cli.command {
        Commands::Export(config) => cmd_export::run(config),

        Commands::Top => plows_top::run().map_err(|e| anyhow::anyhow!("{e}")),

        Commands::Ctl(ctl) => cmd_ctl::run(ctl),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unified_cli_exposes_export_and_control_features() {
        Cli::command().debug_assert();
        assert!(Cli::try_parse_from(["plow-smi", "export", "--interval", "0"]).is_err());
        for args in [
            vec!["plow-smi", "ctl", "capabilities"],
            vec!["plow-smi", "ctl", "apply", "--dry-run"],
            vec!["plow-smi", "ctl", "cpu", "info"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
    }
    use clap::CommandFactory;
}

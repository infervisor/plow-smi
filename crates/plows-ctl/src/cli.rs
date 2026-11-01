//! plows-ctl — GPU power & clock control CLI (via plows-gpu).
//!
//! Usage:
//!   plows-ctl list                             # All vendors
//!   plows-ctl nvidia list | amd list | intel list
//!   plows-ctl nvidia set-power-limit 0 --watts 300
//!   plows-ctl amd set-perf 0 --level high

use crate::{OutputFormat, PerfLevel};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "plows-ctl",
    version,
    about = "GPU list & control CLI (NVIDIA / AMD / Intel via plows-gpu)",
    long_about = "List GPUs across vendors and control NVIDIA/AMD power & clocks. \
                  Requires root/sudo for most set operations."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Apple Silicon telemetry (read-only)
    #[command(subcommand)]
    Apple(AppleCommands),
    /// List every GPU discovered (NVIDIA + AMD + Intel)
    List {
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// NVIDIA GPU control operations
    #[command(subcommand)]
    Nvidia(NvidiaCommands),

    /// AMD GPU control operations
    #[command(subcommand)]
    Amd(AmdCommands),

    /// Intel GPU info (metrics; Level Zero)
    #[command(subcommand)]
    Intel(IntelCommands),

    /// CPU frequency control (cpufreq)
    #[command(subcommand)]
    Cpu(CpuCommands),

    /// Everything this node can control, and what each profile means here
    Capabilities {
        #[arg(long, default_value = "json")]
        format: OutputFormat,
    },

    /// Apply a profile or explicit settings, read as JSON on stdin
    ///
    /// {"profile":"balanced"} or
    /// {"settings":{"cpu":{"governor":"schedutil"},"gpus":{"0":{"power_limit_w":250}}}}
    ///
    /// Exit 0: all applied or already so. 2: partial. 1: nothing done.
    Apply {
        /// Resolve, validate and diff, without writing
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "json")]
        format: OutputFormat,
    },
}

#[derive(Subcommand)]
enum AppleCommands {
    /// List Apple Silicon devices
    List {
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
    /// Show CPU/GPU/Neural Engine and unified-memory metrics
    Info {
        #[arg(default_value_t = 0)]
        gpu: u32,
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
}

#[derive(Subcommand)]
pub enum CpuCommands {
    /// Show cpufreq state
    Info {
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
    /// Set the scaling governor on every policy
    SetGovernor {
        governor: String,
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
    /// Set the maximum frequency (kHz) on every policy
    SetMaxFreq {
        khz: u64,
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
    /// Set energy_performance_preference on every policy
    SetEpp {
        epp: String,
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
}

#[derive(Subcommand)]
enum NvidiaCommands {
    /// List all detected NVIDIA GPUs
    List {
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show detailed info for a specific GPU
    Info {
        /// GPU index (0-based)
        gpu: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Set application clock speeds
    SetClocks {
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
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Set power limit (watts)
    SetPowerLimit {
        /// GPU index (0-based)
        gpu: u32,
        /// Power limit in watts
        #[arg(long)]
        watts: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Set performance level
    SetPerf {
        /// GPU index (0-based), or use --all
        gpu: Option<u32>,
        /// Performance level: auto, low, high
        #[arg(long)]
        level: PerfLevel,
        /// Apply to all GPUs
        #[arg(long)]
        all: bool,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Reset clocks to default
    Reset {
        /// GPU index (0-based)
        gpu: Option<u32>,
        /// Reset all GPUs
        #[arg(long)]
        all: bool,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show supported clock speeds for a GPU
    SupportedClocks {
        /// GPU index (0-based)
        gpu: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
}

#[derive(Subcommand)]
enum AmdCommands {
    /// List all detected AMD GPUs
    List {
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show detailed info for a specific GPU
    Info {
        /// GPU index (0-based)
        gpu: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Set power limit (watts)
    SetPowerLimit {
        /// GPU index (0-based)
        gpu: u32,
        /// Power limit in watts
        #[arg(long)]
        watts: u32,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Set performance level
    SetPerf {
        /// GPU index (0-based), or use --all
        gpu: Option<u32>,
        /// Performance level: auto, low, high
        #[arg(long)]
        level: PerfLevel,
        /// Apply to all GPUs
        #[arg(long)]
        all: bool,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Reset to default settings
    Reset {
        /// GPU index (0-based)
        gpu: Option<u32>,
        /// Reset all GPUs
        #[arg(long)]
        all: bool,
        /// Output format
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
}

#[derive(Subcommand)]
enum IntelCommands {
    /// List all detected Intel GPUs
    List {
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },

    /// Show detailed info for a specific GPU
    Info {
        gpu: u32,
        #[arg(long, default_value = "text")]
        format: OutputFormat,
    },
}

pub fn run() -> anyhow::Result<()> {
    // Logs on stderr: stdout carries results, and the node agent parses it.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if wants_json(&cli.command) {
        colored::control::set_override(false);
    }

    match cli.command {
        Commands::List { format } => crate::list::list_all(&format),
        Commands::Apple(AppleCommands::List { format }) => crate::apple::list_gpus(&format),
        Commands::Apple(AppleCommands::Info { gpu, format }) => {
            crate::apple::gpu_info(gpu, &format)
        }
        Commands::Nvidia(cmd) => run_nvidia(cmd),
        Commands::Amd(cmd) => run_amd(cmd),
        Commands::Intel(cmd) => run_intel(cmd),
        Commands::Cpu(cmd) => run_cpu(cmd),
        Commands::Capabilities { format } => run_capabilities(&format),
        Commands::Apply { dry_run, format } => run_apply(dry_run, &format),
    }
}

fn is_json(f: &OutputFormat) -> bool {
    matches!(f, OutputFormat::Json)
}

fn wants_json(c: &Commands) -> bool {
    match c {
        Commands::Apple(AppleCommands::List { format } | AppleCommands::Info { format, .. }) => {
            is_json(format)
        }
        Commands::Capabilities { format } | Commands::Apply { format, .. } => is_json(format),
        Commands::Cpu(
            CpuCommands::Info { format }
            | CpuCommands::SetGovernor { format, .. }
            | CpuCommands::SetMaxFreq { format, .. }
            | CpuCommands::SetEpp { format, .. },
        ) => is_json(format),
        _ => false,
    }
}

pub fn run_capabilities(format: &OutputFormat) -> anyhow::Result<()> {
    use crate::apply::PowerControl;
    let node = crate::system::SystemControl::load().read();
    let caps = crate::caps::Capabilities::from_node(node);
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&caps)?),
        OutputFormat::Text => crate::report::print_capabilities(&caps),
    }
    Ok(())
}

pub fn run_apply(dry_run: bool, format: &OutputFormat) -> anyhow::Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let req: crate::apply::ApplyRequest = serde_json::from_str(&input)
        .map_err(|e| anyhow::anyhow!("invalid apply request on stdin: {e}"))?;
    if req.profile.is_some() == req.settings.is_some() {
        anyhow::bail!("the request needs exactly one of \"profile\" or \"settings\"");
    }
    let mut ctl = crate::system::SystemControl::load();
    let report = crate::apply::apply(&req, &mut ctl, dry_run);
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        OutputFormat::Text => crate::report::print_apply(&report),
    }
    std::process::exit(report.exit_code());
}

pub fn run_cpu(cmd: CpuCommands) -> anyhow::Result<()> {
    use crate::cpu::CpuFreq;
    let cpu = CpuFreq::system();
    let caps = || {
        cpu.read()
            .ok_or_else(|| anyhow::anyhow!("no cpufreq on this machine"))
    };
    match cmd {
        CpuCommands::Info { format } => {
            let c = caps()?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&c)?),
                OutputFormat::Text => crate::report::print_cpu(&c),
            }
            Ok(())
        }
        CpuCommands::SetGovernor { governor, format } => cpu_set(
            &cpu,
            &format,
            "governor",
            governor.clone().into(),
            |c| c.governor.clone().map(Into::into),
            || cpu.set_governor(&governor),
        ),
        CpuCommands::SetMaxFreq { khz, format } => {
            let c = caps()?;
            if let (Some(lo), Some(hi)) = (c.hw_min_khz, c.hw_max_khz) {
                if khz < lo || khz > hi {
                    anyhow::bail!("{khz} kHz is outside {lo}–{hi} kHz");
                }
            }
            cpu_set(
                &cpu,
                &format,
                "max_freq_khz",
                khz.into(),
                |c| c.max_freq_khz.map(Into::into),
                || cpu.set_max_freq_khz(khz),
            )
        }
        CpuCommands::SetEpp { epp, format } => cpu_set(
            &cpu,
            &format,
            "epp",
            epp.clone().into(),
            |c| c.epp.clone().map(Into::into),
            || cpu.set_epp(&epp),
        ),
    }
}

/// A cpufreq write with read-back, printed like the GPU commands.
fn cpu_set(
    cpu: &crate::cpu::CpuFreq,
    format: &OutputFormat,
    setting: &str,
    to: serde_json::Value,
    read: impl Fn(&crate::cpu::CpuCaps) -> Option<serde_json::Value>,
    write: impl FnOnce() -> std::io::Result<()>,
) -> anyhow::Result<()> {
    use crate::outcome::{Outcome, Status};
    let before = cpu.read().and_then(|c| read(&c));
    write().map_err(|e| {
        anyhow::anyhow!("Failed to set {setting}: {e}. Do you have root/sudo permissions?")
    })?;
    let mut o = Outcome::new("cpu", setting, to.clone())
        .from_value(before)
        .with(Status::Applied, None);
    o.actual = cpu.read().and_then(|c| read(&c));
    let shown = o.actual.clone().unwrap_or(serde_json::Value::Null);
    crate::emit(
        format,
        &o,
        format!("✓ CPU: {setting} set to {to} (now {shown})"),
    )
}

fn run_nvidia(cmd: NvidiaCommands) -> anyhow::Result<()> {
    use crate::nvidia;

    match cmd {
        NvidiaCommands::List { format } => nvidia::list_gpus(&format),
        NvidiaCommands::Info { gpu, format } => nvidia::gpu_info(gpu, &format),
        NvidiaCommands::SetClocks {
            gpu,
            mem,
            graphics,
            all,
            format,
        } => {
            if all {
                nvidia::set_clocks_all(mem, graphics, &format).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                nvidia::set_clocks(idx, mem, graphics, &format).map(drop)
            }
        }
        NvidiaCommands::SetPowerLimit { gpu, watts, format } => {
            nvidia::set_power_limit(gpu, watts, &format).map(drop)
        }
        NvidiaCommands::SetPerf {
            gpu,
            level,
            all,
            format,
        } => {
            if all {
                nvidia::set_perf_all(&level, &format).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                nvidia::set_perf(idx, &level, &format).map(drop)
            }
        }
        NvidiaCommands::Reset { gpu, all, format } => {
            if all {
                nvidia::reset_all(&format).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                nvidia::reset_clocks(idx, &format).map(drop)
            }
        }
        NvidiaCommands::SupportedClocks { gpu, format } => nvidia::supported_clocks(gpu, &format),
    }
}

fn run_amd(cmd: AmdCommands) -> anyhow::Result<()> {
    use crate::amd;

    match cmd {
        AmdCommands::List { format } => amd::list_gpus(&format),
        AmdCommands::Info { gpu, format } => amd::gpu_info(gpu, &format),
        AmdCommands::SetPowerLimit { gpu, watts, format } => {
            amd::set_power_limit(gpu, watts, &format).map(drop)
        }
        AmdCommands::SetPerf {
            gpu,
            level,
            all,
            format,
        } => {
            if all {
                amd::set_perf_all(&level, &format).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                amd::set_perf(idx, &level, &format).map(drop)
            }
        }
        AmdCommands::Reset { gpu, all, format } => {
            if all {
                amd::reset_all(&format).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                amd::reset_clocks(idx, &format).map(drop)
            }
        }
    }
}

fn run_intel(cmd: IntelCommands) -> anyhow::Result<()> {
    use crate::intel;
    match cmd {
        IntelCommands::List { format } => intel::list_gpus(&format),
        IntelCommands::Info { gpu, format } => intel::gpu_info(gpu, &format),
    }
}

//! Ctl subcommand — GPU power & clock control.
//! Delegates entirely to the `plows-ctl` library (DRY principle).

use crate::CtlCommands;
use anyhow::Result;

/// This CLI prints the human text, as it always has.
const TEXT: &plows_ctl::OutputFormat = &plows_ctl::OutputFormat::Text;

pub fn run(cmd: CtlCommands) -> Result<()> {
    match cmd {
        CtlCommands::Capabilities { format } => plows_ctl::cli::run_capabilities(&format),
        CtlCommands::Apply { dry_run, format } => plows_ctl::cli::run_apply(dry_run, &format),
        CtlCommands::Cpu(command) => plows_ctl::cli::run_cpu(command),
        CtlCommands::AppleList { format } => plows_ctl::apple::list_gpus(&format),
        CtlCommands::AppleInfo { gpu, format } => plows_ctl::apple::gpu_info(gpu, &format),
        CtlCommands::List { format } => plows_ctl::list::list_all(&format),
        CtlCommands::NvidiaList { format } => plows_ctl::nvidia::list_gpus(&format),
        CtlCommands::NvidiaInfo { gpu, format } => plows_ctl::nvidia::gpu_info(gpu, &format),
        CtlCommands::NvidiaSetClocks {
            gpu,
            mem,
            graphics,
            all,
        } => {
            if all {
                plows_ctl::nvidia::set_clocks_all(mem, graphics, TEXT).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                plows_ctl::nvidia::set_clocks(idx, mem, graphics, TEXT).map(drop)
            }
        }
        CtlCommands::NvidiaSetPowerLimit { gpu, watts } => {
            plows_ctl::nvidia::set_power_limit(gpu, watts, TEXT).map(drop)
        }
        CtlCommands::NvidiaSetPerf { gpu, level, all } => {
            if all {
                plows_ctl::nvidia::set_perf_all(&level, TEXT).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                plows_ctl::nvidia::set_perf(idx, &level, TEXT).map(drop)
            }
        }
        CtlCommands::NvidiaReset { gpu, all } => {
            if all {
                plows_ctl::nvidia::reset_all(TEXT).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                plows_ctl::nvidia::reset_clocks(idx, TEXT).map(drop)
            }
        }
        CtlCommands::NvidiaSupportedClocks { gpu, format } => {
            plows_ctl::nvidia::supported_clocks(gpu, &format)
        }
        CtlCommands::AmdList { format } => plows_ctl::amd::list_gpus(&format),
        CtlCommands::AmdInfo { gpu, format } => plows_ctl::amd::gpu_info(gpu, &format),
        CtlCommands::AmdSetPowerLimit { gpu, watts } => {
            plows_ctl::amd::set_power_limit(gpu, watts, TEXT).map(drop)
        }
        CtlCommands::AmdSetPerf { gpu, level, all } => {
            if all {
                plows_ctl::amd::set_perf_all(&level, TEXT).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                plows_ctl::amd::set_perf(idx, &level, TEXT).map(drop)
            }
        }
        CtlCommands::AmdReset { gpu, all } => {
            if all {
                plows_ctl::amd::reset_all(TEXT).map(drop)
            } else {
                let idx = gpu.ok_or_else(|| anyhow::anyhow!("Specify GPU index or use --all"))?;
                plows_ctl::amd::reset_clocks(idx, TEXT).map(drop)
            }
        }
        CtlCommands::IntelList { format } => plows_ctl::intel::list_gpus(&format),
        CtlCommands::IntelInfo { gpu, format } => plows_ctl::intel::gpu_info(gpu, &format),
    }
}

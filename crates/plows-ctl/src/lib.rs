//! plows-ctl — GPU power & clock control library.
//!
//! Backed entirely by [`plows_gpu`] (runtime NVML / amdgpu sysfs / Level Zero). Used as a
//! library by `plows-cli` or as the standalone `plows-ctl` binary.

pub mod amd;
pub mod apply;
pub mod caps;
pub mod cpu;
pub mod intel;
pub mod list;
pub mod nvidia;
pub mod outcome;
pub mod profile;
pub mod report;
pub mod system;

use clap::ValueEnum;
use colored::Colorize;
use serde::Serialize;

use outcome::{Outcome, Status};

/// Output format for CLI commands.
#[derive(Clone, Debug, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

/// GPU performance level.
#[derive(Clone, Debug, ValueEnum)]
pub enum PerfLevel {
    Auto,
    Low,
    High,
}

/// Format bytes into a human-readable string (GiB, MiB, or KiB).
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KiB", bytes as f64 / 1024.0)
    }
}

/// Summary entry for a GPU in list output (shared across vendors).
#[derive(Debug, Clone, Serialize)]
pub struct GpuListEntry {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub temperature_c: Option<i64>,
    pub power_w: Option<u64>,
    pub power_limit_w: Option<u64>,
    pub memory_used: String,
    pub memory_total: String,
}

/// Print one outcome: the human line for `text`, the outcome for `json`.
pub fn emit(format: &OutputFormat, outcome: &Outcome, text: String) -> anyhow::Result<()> {
    match format {
        OutputFormat::Text => println!("{}", text.green()),
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(outcome)?),
    }
    Ok(())
}

/// For `--all` commands: text lines are printed as each GPU finishes; JSON is
/// one array at the end, so stdout is a single document.
pub fn emit_all(format: &OutputFormat, outcomes: &[Outcome]) -> anyhow::Result<()> {
    if let OutputFormat::Json = format {
        println!("{}", serde_json::to_string_pretty(outcomes)?);
    }
    let failures = outcomes
        .iter()
        .filter(|o| {
            matches!(
                o.status,
                Status::Failed | Status::Refused | Status::Unsupported
            )
        })
        .count();
    anyhow::ensure!(failures == 0, "{failures} GPU operation(s) failed");
    Ok(())
}

/// A green line in text mode; nothing in JSON mode.
pub fn text_line(format: &OutputFormat, text: String) {
    if let OutputFormat::Text = format {
        println!("{}", text.green());
    }
}

/// The outcome for a GPU whose write failed.
pub fn failed(
    index: u32,
    setting: &str,
    to: impl Into<serde_json::Value>,
    err: &anyhow::Error,
) -> Outcome {
    Outcome::new(format!("gpu{index}"), setting, to).with(Status::Failed, Some(err.to_string()))
}
pub mod apple;

pub mod cli;

/// Human-readable optional telemetry.
pub fn value_or_na(value: Option<impl std::fmt::Display>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(|| "N/A".into())
}

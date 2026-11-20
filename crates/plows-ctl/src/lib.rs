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

/// Placeholder for telemetry a device does not report.
pub const NOT_AVAILABLE: &str = "N/A";

impl GpuListEntry {
    /// Table cell for the temperature, e.g. `46°C`.
    pub fn temperature_cell(&self) -> String {
        self.temperature_c
            .map_or_else(|| NOT_AVAILABLE.into(), |t| format!("{t}°C"))
    }

    /// Table cell for power draw and limit, e.g. `4/140W`, or `4W` when the
    /// device has no power limit (integrated and SoC GPUs).
    pub fn power_cell(&self) -> String {
        match (self.power_w, self.power_limit_w) {
            (Some(power), Some(limit)) => format!("{power}/{limit}W"),
            (Some(power), None) => format!("{power}W"),
            (None, Some(limit)) => format!("{NOT_AVAILABLE}/{limit}W"),
            (None, None) => NOT_AVAILABLE.into(),
        }
    }

    /// Table cell for memory, e.g. `2.0 GiB/8.0 GiB`; a single `N/A` when the
    /// device reports no dedicated memory (unified-memory SoCs).
    pub fn memory_cell(&self) -> String {
        if self.memory_used == NOT_AVAILABLE && self.memory_total == NOT_AVAILABLE {
            NOT_AVAILABLE.into()
        } else {
            format!("{}/{}", self.memory_used, self.memory_total)
        }
    }
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

#[cfg(test)]
mod list_entry_tests {
    use super::*;

    fn entry(power: Option<u64>, limit: Option<u64>, memory: Option<(&str, &str)>) -> GpuListEntry {
        let (used, total) = memory.unwrap_or((NOT_AVAILABLE, NOT_AVAILABLE));
        GpuListEntry {
            index: 0,
            name: "NVIDIA GB10".into(),
            uuid: "uuid".into(),
            temperature_c: None,
            power_w: power,
            power_limit_w: limit,
            memory_used: used.into(),
            memory_total: total.into(),
        }
    }

    #[test]
    fn unified_memory_soc_cells_collapse_to_a_single_placeholder() {
        let e = entry(Some(4), None, None);
        assert_eq!(e.power_cell(), "4W");
        assert_eq!(e.memory_cell(), "N/A");
        assert_eq!(e.temperature_cell(), "N/A");
    }

    #[test]
    fn discrete_gpu_cells_show_both_values() {
        let mut e = entry(Some(90), Some(140), Some(("2.0 GiB", "8.0 GiB")));
        e.temperature_c = Some(46);
        assert_eq!(e.power_cell(), "90/140W");
        assert_eq!(e.memory_cell(), "2.0 GiB/8.0 GiB");
        assert_eq!(e.temperature_cell(), "46°C");
        assert_eq!(entry(None, Some(140), None).power_cell(), "N/A/140W");
        assert_eq!(entry(None, None, None).power_cell(), "N/A");
    }
}

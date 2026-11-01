//! AMD GPU control via `plows-gpu` (runtime AMD SMI).

use anyhow::{bail, Result};
use colored::Colorize;
use plows_gpu::{AmdBackend, GpuBackend};

use crate::outcome::{Outcome, Status};
use crate::{
    emit, emit_all, failed, format_bytes, text_line, GpuListEntry, OutputFormat, PerfLevel,
};

fn init_backend() -> Result<AmdBackend> {
    AmdBackend::try_load().map_err(|e| {
        anyhow::anyhow!(
            "Failed to initialize AMD SMI via plows-gpu.\n\
             Ensure AMD GPU drivers and ROCm are installed.\n\
             Hint: Run 'rocm-smi' to check driver status.\nError: {e}"
        )
    })
}

pub fn list_gpus(format: &OutputFormat) -> Result<()> {
    let mut backend = init_backend()?;
    backend.refresh();
    let count = backend.device_count() as u32;
    if count == 0 {
        bail!("No AMD GPUs detected");
    }

    let mut entries = Vec::new();
    for (i, device) in backend.devices().into_iter().enumerate() {
        let m = backend.snapshot(i).unwrap_or_default();
        entries.push(GpuListEntry {
            index: i as u32,
            name: device.model,
            uuid: device.uuid,
            temperature_c: m.temperature.map(|t| t as i64),
            power_w: m.power_usage.map(|w| w as u64),
            power_limit_w: m.power_limit.map(|w| w as u64),
            memory_used: m
                .memory_used
                .map(format_bytes)
                .unwrap_or_else(|| "N/A".into()),
            memory_total: m
                .memory_total
                .map(format_bytes)
                .unwrap_or_else(|| "N/A".into()),
        });
    }

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&entries)?),
        OutputFormat::Text => {
            println!("{}", format!("AMD GPUs detected: {count}").green());
            println!("{}", "─".repeat(80));
            println!(
                "{:>3} {:30} {:>6} {:>12} {:>16}",
                "ID".bold(),
                "Name".bold(),
                "Temp".bold(),
                "Power".bold(),
                "Memory".bold()
            );
            println!("{}", "─".repeat(80));
            for e in &entries {
                println!(
                    "{:>3} {:30} {:>4}°C {:>5}/{:<5}W {:>16}",
                    e.index.to_string().cyan(),
                    e.name,
                    crate::value_or_na(e.temperature_c),
                    crate::value_or_na(e.power_w),
                    crate::value_or_na(e.power_limit_w),
                    format!("{}/{}", e.memory_used, e.memory_total),
                );
            }
        }
    }
    Ok(())
}

pub fn gpu_info(index: u32, format: &OutputFormat) -> Result<()> {
    let mut backend = init_backend()?;
    backend.refresh();
    if index as usize >= backend.device_count() {
        bail!(
            "AMD GPU {index} not found (detected {} devices)",
            backend.device_count()
        );
    }
    let device = backend.devices().into_iter().nth(index as usize).unwrap();
    let m = backend.snapshot(index as usize).unwrap_or_default();
    let perf_str = match backend.perf_level(index) {
        Some(0) => "auto",
        Some(1) => "low",
        Some(2) => "high",
        Some(3) => "manual",
        _ => "unknown",
    };

    match format {
        OutputFormat::Json => {
            let info = serde_json::json!({
                "index": index,
                "name": device.model,
                "uuid": device.uuid,
                "temperature_c": m.temperature,
                "power_w": m.power_usage,
                "power_limit_w": m.power_limit,
                "memory_used_bytes": m.memory_used,
                "memory_total_bytes": m.memory_total,
                "clock_graphics_mhz": m.clock_graphics,
                "clock_memory_mhz": m.clock_memory,
                "fan_speed_rpm": m.fan_speed,
                "perf_level": perf_str,
            });
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        OutputFormat::Text => {
            println!(
                "{}",
                format!("AMD GPU {index}: {}", device.model).cyan().bold()
            );
            println!("{}", "─".repeat(50));
            println!("  UUID:          {}", device.uuid);
            println!("  Temperature:   {}°C", m.temperature.unwrap_or(0.0));
            println!(
                "  Power:         {:.0} / {:.0} W",
                m.power_usage.unwrap_or(0.0),
                m.power_limit.unwrap_or(0.0)
            );
            if let (Some(used), Some(total)) = (m.memory_used, m.memory_total) {
                println!(
                    "  Memory:        {} / {}",
                    format_bytes(used),
                    format_bytes(total)
                );
            }
            println!("  Core Clock:    {} MHz", m.clock_graphics.unwrap_or(0));
            println!("  Memory Clock:  {} MHz", m.clock_memory.unwrap_or(0));
            if let Some(f) = m.fan_speed {
                println!("  Fan:           {f} RPM");
            }
            println!("  Perf Level:    {perf_str}");
        }
    }
    Ok(())
}

fn check_index(backend: &AmdBackend, index: u32) -> Result<()> {
    if index as usize >= backend.device_count() {
        bail!(
            "AMD GPU {index} not found (detected {} devices)",
            backend.device_count()
        );
    }
    Ok(())
}

fn perf_name(code: Option<u32>) -> Option<&'static str> {
    match code {
        Some(0) => Some("auto"),
        Some(1) => Some("low"),
        Some(2) => Some("high"),
        Some(3) => Some("manual"),
        _ => None,
    }
}

fn current_watts(backend: &AmdBackend, index: u32) -> Option<serde_json::Value> {
    let mw = backend.power_limits(index).ok()?.current_mw?;
    Some(serde_json::json!(mw / 1000))
}

fn power_limit_outcome(backend: &AmdBackend, index: u32, watts: u32) -> Result<Outcome> {
    check_index(backend, index)?;
    let limits = backend.power_limits(index).unwrap_or_default();
    let milliwatts = watts as u64 * 1000;
    // A zero cap is never meaningful; outside min/max the driver would refuse.
    let lo = limits.min_mw.unwrap_or(0).max(1);
    if milliwatts < lo || limits.max_mw.is_some_and(|hi| milliwatts > hi) {
        bail!(
            "Power limit {watts}W out of range. Valid: {} - {} W",
            lo.div_ceil(1000),
            limits
                .max_mw
                .map_or("?".to_string(), |m| (m / 1000).to_string())
        );
    }
    let before = current_watts(backend, index);
    backend.set_power_limit(index, milliwatts).map_err(|e| {
        anyhow::anyhow!(
            "Failed to set power limit for AMD GPU {index}: {e}\n\
             Do you have root/sudo permissions?"
        )
    })?;
    let mut o = Outcome::new(format!("gpu{index}"), "power_limit_w", watts)
        .from_value(before)
        .with(Status::Applied, None);
    o.actual = current_watts(backend, index);
    Ok(o)
}

pub fn set_power_limit(index: u32, watts: u32, format: &OutputFormat) -> Result<Outcome> {
    let backend = init_backend()?;
    let o = power_limit_outcome(&backend, index, watts)?;
    emit(
        format,
        &o,
        format!("✓ AMD GPU {index}: Power limit set to {watts}W"),
    )?;
    Ok(o)
}

fn perf_outcome(backend: &AmdBackend, index: u32, level_str: &str) -> Result<Outcome> {
    check_index(backend, index)?;
    let before = perf_name(backend.perf_level(index));
    backend.set_perf_level(index, level_str).map_err(|e| {
        anyhow::anyhow!(
            "Failed to set performance level for AMD GPU {index}: {e}\n\
             Do you have root/sudo permissions?"
        )
    })?;
    let mut o = Outcome::new(format!("gpu{index}"), "perf_level", level_str)
        .from_value(before.map(Into::into))
        .with(Status::Applied, None);
    o.actual = perf_name(backend.perf_level(index)).map(Into::into);
    Ok(o)
}

fn level_str(level: &PerfLevel) -> &'static str {
    match level {
        PerfLevel::Auto => "auto",
        PerfLevel::Low => "low",
        PerfLevel::High => "high",
    }
}

pub fn set_perf(index: u32, level: &PerfLevel, format: &OutputFormat) -> Result<Outcome> {
    let backend = init_backend()?;
    let l = level_str(level);
    let o = perf_outcome(&backend, index, l)?;
    emit(
        format,
        &o,
        format!("✓ AMD GPU {index}: Performance set to {}", l.to_uppercase()),
    )?;
    Ok(o)
}

pub fn set_perf_all(level: &PerfLevel, format: &OutputFormat) -> Result<Vec<Outcome>> {
    let backend = init_backend()?;
    let l = level_str(level);
    let mut out = Vec::new();
    for i in 0..backend.device_count() as u32 {
        match perf_outcome(&backend, i, l) {
            Ok(o) => {
                text_line(
                    format,
                    format!("✓ AMD GPU {i}: Performance set to {}", l.to_uppercase()),
                );
                out.push(o);
            }
            Err(e) => {
                eprintln!("{}", format!("✗ AMD GPU {i}: {e}").red());
                out.push(failed(i, "perf_level", l, &e));
            }
        }
    }
    emit_all(format, &out)?;
    Ok(out)
}

pub fn reset_clocks(index: u32, format: &OutputFormat) -> Result<Outcome> {
    let backend = init_backend()?;
    let o = perf_outcome(&backend, index, "auto")?;
    emit(
        format,
        &o,
        format!("✓ AMD GPU {index}: Reset to defaults (auto perf level)"),
    )?;
    Ok(o)
}

pub fn reset_all(format: &OutputFormat) -> Result<Vec<Outcome>> {
    let backend = init_backend()?;
    let mut out = Vec::new();
    for i in 0..backend.device_count() as u32 {
        match perf_outcome(&backend, i, "auto") {
            Ok(o) => {
                text_line(
                    format,
                    format!("✓ AMD GPU {i}: Reset to defaults (auto perf level)"),
                );
                out.push(o);
            }
            Err(e) => {
                eprintln!("{}", format!("✗ AMD GPU {i}: {e}").red());
                out.push(failed(i, "perf_level", "auto", &e));
            }
        }
    }
    text_line(format, "✓ All AMD GPUs reset to defaults".to_string());
    emit_all(format, &out)?;
    Ok(out)
}

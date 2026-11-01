//! NVIDIA GPU control via `plows-gpu` (runtime NVML).

use anyhow::{bail, Context, Result};
use colored::Colorize;
use plows_gpu::{GpuBackend, NvidiaBackend};

use crate::outcome::{Outcome, Status};
use crate::{
    emit, emit_all, failed, format_bytes, text_line, GpuListEntry, OutputFormat, PerfLevel,
};

fn init_backend() -> Result<NvidiaBackend> {
    NvidiaBackend::try_load().map_err(|e| {
        anyhow::anyhow!(
            "Failed to initialize NVML via plows-gpu. Ensure NVIDIA drivers are installed.\n\
             Hint: Run 'nvidia-smi' to check driver status.\nError: {e}"
        )
    })
}

pub fn list_gpus(format: &OutputFormat) -> Result<()> {
    let mut backend = init_backend()?;
    backend.refresh();
    let count = backend.device_count() as u32;
    if count == 0 {
        bail!("No NVIDIA GPUs detected");
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
            memory_used: m.memory_used.map(format_bytes).unwrap_or_default(),
            memory_total: m.memory_total.map(format_bytes).unwrap_or_default(),
        });
    }

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&entries)?),
        OutputFormat::Text => {
            let driver = backend.driver_version().unwrap_or_else(|| "N/A".into());
            println!("{}", format!("NVIDIA Driver: {driver}").cyan());
            println!("{}", format!("GPUs detected: {count}").green());
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
    let device = backend
        .devices()
        .into_iter()
        .nth(index as usize)
        .ok_or_else(|| anyhow::anyhow!("GPU {index} not found"))?;
    let m = backend.snapshot(index as usize).unwrap_or_default();
    let (max_core, max_mem) = backend.max_clocks(index).unwrap_or((0, 0));
    let (min_power, max_power) = backend
        .power_limit_constraints(index)
        .map(|(min, max)| (Some(min), Some(max)))
        .unwrap_or((None, None));
    let (pcie_gen, pcie_width) = backend.pcie_info(index).unwrap_or((None, None));

    match format {
        OutputFormat::Json => {
            let info = serde_json::json!({
                "index": index,
                "name": device.model,
                "uuid": device.uuid,
                "temperature_c": m.temperature,
                "power_w": m.power_usage,
                "power_limit_w": m.power_limit,
                "power_min_mw": min_power,
                "power_max_mw": max_power,
                "memory_used_bytes": m.memory_used,
                "memory_total_bytes": m.memory_total,
                "clock_graphics_mhz": m.clock_graphics,
                "clock_memory_mhz": m.clock_memory,
                "max_clock_graphics_mhz": max_core,
                "max_clock_memory_mhz": max_mem,
                "fan_speed_percent": m.fan_speed,
                "pcie_gen": pcie_gen,
                "pcie_width": pcie_width,
            });
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        OutputFormat::Text => {
            println!("{}", format!("GPU {index}: {}", device.model).cyan().bold());
            println!("{}", "─".repeat(50));
            println!("  UUID:          {}", device.uuid);
            println!("  Temperature:   {}°C", m.temperature.unwrap_or(0.0));
            println!(
                "  Power:         {:.0} / {:.0} W",
                m.power_usage.unwrap_or(0.0),
                m.power_limit.unwrap_or(0.0)
            );
            if let (Some(min), Some(max)) = (min_power, max_power) {
                println!("  Power Range:   {} - {} W", min / 1000, max / 1000);
            }
            if let (Some(used), Some(total)) = (m.memory_used, m.memory_total) {
                println!(
                    "  Memory:        {} / {}",
                    format_bytes(used),
                    format_bytes(total)
                );
            }
            println!(
                "  Core Clock:    {} MHz (max: {max_core} MHz)",
                m.clock_graphics.unwrap_or(0)
            );
            println!(
                "  Memory Clock:  {} MHz (max: {max_mem} MHz)",
                m.clock_memory.unwrap_or(0)
            );
            if let Some(f) = m.fan_speed {
                println!("  Fan:           {f}%");
            }
            if let (Some(gen), Some(width)) = (pcie_gen, pcie_width) {
                println!("  PCIe:          Gen{gen} x{width}");
            }
        }
    }
    Ok(())
}

fn clocks_value(clocks: Option<(u32, u32)>) -> Option<serde_json::Value> {
    clocks.map(|(mem, gfx)| serde_json::json!({ "memory_mhz": mem, "graphics_mhz": gfx }))
}

fn current_watts(backend: &NvidiaBackend, index: u32) -> Option<serde_json::Value> {
    let mw = backend.power_limits(index).ok()?.current_mw?;
    Some(serde_json::json!(mw / 1000))
}

fn clocks_outcome(
    backend: &NvidiaBackend,
    index: u32,
    mem_clk: u32,
    graphics_clk: u32,
) -> Result<Outcome> {
    let supported_mem = backend
        .supported_memory_clocks(index)
        .context("Failed to query supported memory clocks")?;
    if !supported_mem.contains(&mem_clk) {
        bail!(
            "Memory clock {mem_clk} MHz not supported.\nSupported: {supported_mem:?}\n\
             Hint: Use 'plows-ctl nvidia supported-clocks {index}' to see valid combinations."
        );
    }
    let supported_gfx = backend
        .supported_graphics_clocks(index, mem_clk)
        .context("Failed to query supported graphics clocks")?;
    if !supported_gfx.contains(&graphics_clk) {
        bail!(
            "Graphics clock {graphics_clk} MHz not supported for mem={mem_clk} MHz.\n\
             Supported range: {} - {} MHz",
            supported_gfx.last().unwrap_or(&0),
            supported_gfx.first().unwrap_or(&0),
        );
    }
    let before = clocks_value(backend.applications_clocks(index));
    backend
        .set_applications_clocks(index, mem_clk, graphics_clk)
        .context("Failed to set application clocks. Do you have root/sudo permissions?")?;
    let mut o = Outcome::new(
        format!("gpu{index}"),
        "applications_clocks",
        serde_json::json!({ "memory_mhz": mem_clk, "graphics_mhz": graphics_clk }),
    )
    .from_value(before)
    .with(Status::Applied, None);
    o.actual = clocks_value(backend.applications_clocks(index));
    Ok(o)
}

pub fn set_clocks(
    index: u32,
    mem_clk: u32,
    graphics_clk: u32,
    format: &OutputFormat,
) -> Result<Outcome> {
    let backend = init_backend()?;
    let o = clocks_outcome(&backend, index, mem_clk, graphics_clk)?;
    emit(
        format,
        &o,
        format!("✓ GPU {index}: Set clocks to mem={mem_clk} MHz, graphics={graphics_clk} MHz"),
    )?;
    Ok(o)
}

pub fn set_clocks_all(
    mem_clk: u32,
    graphics_clk: u32,
    format: &OutputFormat,
) -> Result<Vec<Outcome>> {
    let backend = init_backend()?;
    let mut out = Vec::new();
    for i in 0..backend.device_count() as u32 {
        match clocks_outcome(&backend, i, mem_clk, graphics_clk) {
            Ok(o) => {
                text_line(
                    format,
                    format!(
                        "✓ GPU {i}: Set clocks to mem={mem_clk} MHz, graphics={graphics_clk} MHz"
                    ),
                );
                out.push(o);
            }
            Err(e) => {
                eprintln!("{}", format!("✗ GPU {i}: {e}").red());
                out.push(failed(
                    i,
                    "applications_clocks",
                    serde_json::json!({ "memory_mhz": mem_clk, "graphics_mhz": graphics_clk }),
                    &e,
                ));
            }
        }
    }
    emit_all(format, &out)?;
    Ok(out)
}

pub fn set_power_limit(index: u32, watts: u32, format: &OutputFormat) -> Result<Outcome> {
    let backend = init_backend()?;
    let milliwatts = watts
        .checked_mul(1000)
        .filter(|v| *v > 0)
        .ok_or_else(|| anyhow::anyhow!("power limit must be 1..=4294967 watts"))?;
    if let Ok((min, max)) = backend.power_limit_constraints(index) {
        if milliwatts < min || milliwatts > max {
            bail!(
                "Power limit {watts}W out of range. Valid: {} - {} W",
                min / 1000,
                max / 1000
            );
        }
    }
    let before = current_watts(&backend, index);
    backend
        .set_power_limit(index, milliwatts)
        .context("Failed to set power limit. Do you have root/sudo permissions?")?;
    let mut o = Outcome::new(format!("gpu{index}"), "power_limit_w", watts)
        .from_value(before)
        .with(Status::Applied, None);
    o.actual = current_watts(&backend, index);
    emit(
        format,
        &o,
        format!("✓ GPU {index}: Power limit set to {watts}W"),
    )?;
    Ok(o)
}

/// Resolve a perf level to application clocks, or `None` for auto.
fn perf_clocks(
    backend: &NvidiaBackend,
    index: u32,
    level: &PerfLevel,
) -> Result<Option<(u32, u32)>> {
    let pick_high = match level {
        PerfLevel::Auto => return Ok(None),
        PerfLevel::High => true,
        PerfLevel::Low => false,
    };
    let supported_mem = backend
        .supported_memory_clocks(index)
        .context("Could not determine supported clocks")?;
    let mem = if pick_high {
        supported_mem.first()
    } else {
        supported_mem.last()
    };
    let mem_clk = *mem.ok_or_else(|| anyhow::anyhow!("No supported memory clocks found"))?;
    let supported_gfx = backend
        .supported_graphics_clocks(index, mem_clk)
        .context("Could not determine supported graphics clocks")?;
    let gfx = if pick_high {
        supported_gfx.first()
    } else {
        supported_gfx.last()
    };
    let gfx_clk = *gfx.ok_or_else(|| anyhow::anyhow!("No supported graphics clocks found"))?;
    Ok(Some((mem_clk, gfx_clk)))
}

fn perf_outcome(
    backend: &NvidiaBackend,
    index: u32,
    level: &PerfLevel,
) -> Result<(Outcome, String)> {
    let before = clocks_value(backend.applications_clocks(index));
    let (setting_to, line) = match perf_clocks(backend, index, level)? {
        None => {
            backend
                .reset_applications_clocks(index)
                .context("Failed to reset to auto. Need root?")?;
            (
                serde_json::json!("auto"),
                format!("✓ GPU {index}: Performance set to AUTO (default clocks)"),
            )
        }
        Some((mem_clk, gfx_clk)) => {
            let (name, ctx) = match level {
                PerfLevel::High => ("HIGH", "Failed to set high perf clocks. Need root?"),
                _ => ("LOW", "Failed to set low perf clocks. Need root?"),
            };
            backend
                .set_applications_clocks(index, mem_clk, gfx_clk)
                .context(ctx)?;
            (
                serde_json::json!({ "memory_mhz": mem_clk, "graphics_mhz": gfx_clk }),
                format!(
                    "✓ GPU {index}: Performance set to {name} (mem={mem_clk}, gfx={gfx_clk} MHz)"
                ),
            )
        }
    };
    let mut o = Outcome::new(format!("gpu{index}"), "applications_clocks", setting_to)
        .from_value(before)
        .with(Status::Applied, None);
    o.actual = clocks_value(backend.applications_clocks(index));
    Ok((o, line))
}

pub fn set_perf(index: u32, level: &PerfLevel, format: &OutputFormat) -> Result<Outcome> {
    let backend = init_backend()?;
    let (o, line) = perf_outcome(&backend, index, level)?;
    emit(format, &o, line)?;
    Ok(o)
}

pub fn set_perf_all(level: &PerfLevel, format: &OutputFormat) -> Result<Vec<Outcome>> {
    let backend = init_backend()?;
    let mut out = Vec::new();
    for i in 0..backend.device_count() as u32 {
        match perf_outcome(&backend, i, level) {
            Ok((o, line)) => {
                text_line(format, line);
                out.push(o);
            }
            Err(e) => {
                eprintln!("{}", format!("✗ GPU {i}: {e}").red());
                out.push(failed(
                    i,
                    "applications_clocks",
                    format!("{level:?}").to_lowercase(),
                    &e,
                ));
            }
        }
    }
    emit_all(format, &out)?;
    Ok(out)
}

fn reset_outcome(backend: &NvidiaBackend, index: u32) -> Result<Outcome> {
    let before = clocks_value(backend.applications_clocks(index));
    backend
        .reset_applications_clocks(index)
        .context("Failed to reset clocks. Do you have root/sudo permissions?")?;
    let mut o = Outcome::new(format!("gpu{index}"), "applications_clocks", "default")
        .from_value(before)
        .with(Status::Applied, None);
    o.actual = clocks_value(backend.applications_clocks(index));
    Ok(o)
}

pub fn reset_clocks(index: u32, format: &OutputFormat) -> Result<Outcome> {
    let backend = init_backend()?;
    let o = reset_outcome(&backend, index)?;
    emit(
        format,
        &o,
        format!("✓ GPU {index}: Clocks reset to default"),
    )?;
    Ok(o)
}

pub fn reset_all(format: &OutputFormat) -> Result<Vec<Outcome>> {
    let backend = init_backend()?;
    let mut out = Vec::new();
    for i in 0..backend.device_count() as u32 {
        match reset_outcome(&backend, i) {
            Ok(o) => {
                text_line(format, format!("✓ GPU {i}: Clocks reset to default"));
                out.push(o);
            }
            Err(e) => {
                eprintln!("{}", format!("✗ GPU {i}: {e}").red());
                out.push(failed(i, "applications_clocks", "default", &e));
            }
        }
    }
    emit_all(format, &out)?;
    Ok(out)
}

pub fn supported_clocks(index: u32, format: &OutputFormat) -> Result<()> {
    let backend = init_backend()?;
    let supported_mem = backend
        .supported_memory_clocks(index)
        .context("Failed to query supported memory clocks")?;
    let device_name = backend
        .devices()
        .into_iter()
        .nth(index as usize)
        .map(|d| d.model)
        .unwrap_or_else(|| "Unknown".into());

    match format {
        OutputFormat::Json => {
            let mut clock_map = Vec::new();
            for &mem in &supported_mem {
                let gfx = backend
                    .supported_graphics_clocks(index, mem)
                    .unwrap_or_default();
                clock_map.push(serde_json::json!({
                    "memory_mhz": mem,
                    "graphics_mhz_range": [gfx.last(), gfx.first()],
                    "graphics_count": gfx.len(),
                }));
            }
            println!("{}", serde_json::to_string_pretty(&clock_map)?);
        }
        OutputFormat::Text => {
            println!(
                "{}",
                format!("GPU {index}: {device_name} — Supported Clocks")
                    .cyan()
                    .bold()
            );
            println!("{}", "─".repeat(60));
            println!(
                "{:>12} {:>15} {:>15} {:>8}",
                "Mem (MHz)".bold(),
                "GFX Min".bold(),
                "GFX Max".bold(),
                "Steps".bold()
            );
            println!("{}", "─".repeat(60));
            for &mem in &supported_mem {
                let gfx = backend
                    .supported_graphics_clocks(index, mem)
                    .unwrap_or_default();
                let min = gfx.last().copied().unwrap_or(0);
                let max = gfx.first().copied().unwrap_or(0);
                println!("{:>12} {:>15} {:>15} {:>8}", mem, min, max, gfx.len());
            }
        }
    }
    Ok(())
}

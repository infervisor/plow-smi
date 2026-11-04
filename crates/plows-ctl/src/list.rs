//! Cross-vendor list via `GpuManager` (NVIDIA + AMD + Intel).

use anyhow::Result;
use colored::Colorize;
use plows_gpu::GpuManager;

use crate::{format_bytes, GpuListEntry, OutputFormat};

/// List every GPU discovered by `plows-gpu`.
pub fn list_all(format: &OutputFormat) -> Result<()> {
    let mut mgr = GpuManager::discover();
    if !mgr.has_gpu() {
        anyhow::bail!("No GPUs discovered (no NVML, amdgpu driver or Level Zero)");
    }
    mgr.refresh_all();

    let mut entries = Vec::new();
    let mut global = 0u32;
    for backend in mgr.backends() {
        let vendor = backend.vendor().as_str();
        for (id, device) in backend.devices().into_iter().enumerate() {
            let m = backend.snapshot(id).unwrap_or_default();
            entries.push((
                vendor,
                GpuListEntry {
                    index: global,
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
                },
            ));
            global += 1;
        }
    }

    match format {
        OutputFormat::Json => {
            let json: Vec<_> = entries
                .iter()
                .map(|(vendor, e)| {
                    serde_json::json!({
                        "index": e.index,
                        "vendor": vendor,
                        "name": e.name,
                        "uuid": e.uuid,
                        "temperature_c": e.temperature_c,
                        "power_w": e.power_w,
                        "power_limit_w": e.power_limit_w,
                        "memory_used": e.memory_used,
                        "memory_total": e.memory_total,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        OutputFormat::Text => {
            println!(
                "{}",
                format!("{} GPU(s) across all vendors", entries.len()).green()
            );
            println!("{}", "─".repeat(90));
            println!(
                "{:>3} {:8} {:28} {:>6} {:>12} {:>16}",
                "ID".bold(),
                "Vendor".bold(),
                "Name".bold(),
                "Temp".bold(),
                "Power".bold(),
                "Memory".bold()
            );
            println!("{}", "─".repeat(90));
            for (vendor, e) in &entries {
                println!(
                    "{:>3} {:8} {:28} {:>4}°C {:>5}/{:<5}W {:>16}",
                    e.index.to_string().cyan(),
                    vendor,
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

//! Intel GPU list/info via `plows-gpu` (Level Zero — metrics only).

use anyhow::{bail, Result};
use colored::Colorize;
use plows_gpu::{GpuBackend, IntelBackend};

use crate::{format_bytes, GpuListEntry, OutputFormat};

fn init_backend() -> Result<IntelBackend> {
    IntelBackend::try_load().map_err(|e| {
        anyhow::anyhow!(
            "Failed to initialize Level Zero via plows-gpu.\n\
             Ensure Intel GPU drivers / oneAPI Level Zero are installed.\nError: {e}"
        )
    })
}

pub fn list_gpus(format: &OutputFormat) -> Result<()> {
    let mut backend = init_backend()?;
    backend.refresh();
    let count = backend.device_count() as u32;
    if count == 0 {
        bail!("No Intel GPUs detected");
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
            println!("{}", format!("Intel GPUs detected: {count}").green());
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
            "Intel GPU {index} not found (detected {} devices)",
            backend.device_count()
        );
    }
    let device = backend.devices().into_iter().nth(index as usize).unwrap();
    let m = backend.snapshot(index as usize).unwrap_or_default();

    match format {
        OutputFormat::Json => {
            let info = serde_json::json!({
                "index": index,
                "name": device.model,
                "uuid": device.uuid,
                "pci_bus_id": device.pci_bus_id,
                "temperature_c": m.temperature,
                "power_w": m.power_usage,
                "memory_used_bytes": m.memory_used,
                "memory_total_bytes": m.memory_total,
                "clock_graphics_mhz": m.clock_graphics,
                "utilization": m.utilization,
            });
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        OutputFormat::Text => {
            println!(
                "{}",
                format!("Intel GPU {index}: {}", device.model).cyan().bold()
            );
            println!("{}", "─".repeat(50));
            println!("  UUID:          {}", device.uuid);
            if !device.pci_bus_id.is_empty() {
                println!("  PCI:           {}", device.pci_bus_id);
            }
            println!("  Temperature:   {}°C", m.temperature.unwrap_or(0.0));
            println!("  Power:         {:.0} W", m.power_usage.unwrap_or(0.0));
            if let (Some(used), Some(total)) = (m.memory_used, m.memory_total) {
                println!(
                    "  Memory:        {} / {}",
                    format_bytes(used),
                    format_bytes(total)
                );
            }
            println!("  Core Clock:    {} MHz", m.clock_graphics.unwrap_or(0));
            if let Some(u) = m.utilization {
                println!("  Utilization:   {u:.1}%");
            }
        }
    }
    Ok(())
}

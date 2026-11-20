//! System metrics collector (CPU, memory, disk, network, temperature).

use async_trait::async_trait;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{Components, Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System};
use tracing::debug;

use crate::collector::{Collector, GpuSnapshot};
use crate::error::Result;

/// Collected system-level metrics snapshot.
#[derive(Debug, Clone)]
pub struct SystemSnapshot {
    pub hostname: String,
    pub device_name: String,
    pub cpu_usage_percent: f64,
    pub cpu_core_count: Option<usize>,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub memory_available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub uptime_seconds: u64,
    pub load_avg_1m: f64,
    pub load_avg_5m: f64,
    pub load_avg_15m: f64,
    pub process_count: usize,
    pub disks: Vec<DiskInfo>,
    pub networks: Vec<NetworkInfo>,
    pub temperatures: Vec<TemperatureInfo>,
}

/// Disk information.
#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
}

/// Network interface information.
#[derive(Debug, Clone)]
pub struct NetworkInfo {
    pub interface: String,
    pub mac_address: String,
    pub received_bytes: u64,
    pub transmitted_bytes: u64,
    pub total_received_bytes: u64,
    pub total_transmitted_bytes: u64,
}

/// Hardware temperature sensor info.
#[derive(Debug, Clone)]
pub struct TemperatureInfo {
    pub label: String,
    pub current_celsius: f64,
    pub max_celsius: Option<f64>,
}

/// System metrics collector.
pub struct SystemCollector {
    hostname: String,
    device_name: String,
    state: Mutex<SystemState>,
}

struct SystemState {
    sys: System,
    disks: Disks,
    networks: Networks,
    components: Components,
    cores: Option<usize>,
    process_count: usize,
    inventory_refreshed: Option<Instant>,
    sensors_rescanned: Option<Instant>,
}

/// How often disks, sensor readings and the process count are re-read.
const INVENTORY_INTERVAL: Duration = Duration::from_secs(5);
/// How often the set of hardware sensors is re-discovered. Readings are still
/// taken every [`INVENTORY_INTERVAL`]; only hot-plugged sensors wait this long.
const SENSOR_RESCAN_INTERVAL: Duration = Duration::from_secs(60);

/// Number of processes on the host.
///
/// On Linux this counts the numeric entries of `/proc` (one `getdents` pass)
/// instead of letting `sysinfo` open and parse `stat` and `status` for every
/// process, which costs thousands of syscalls per refresh on a busy host.
fn count_processes(sys: &mut System) -> usize {
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/proc") {
        return entries
            .flatten()
            .filter(|e| {
                e.file_name()
                    .as_encoded_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_digit)
            })
            .count();
    }
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().without_tasks(),
    );
    sys.processes().len()
}

impl SystemCollector {
    /// Create a new system collector.
    pub fn new() -> std::result::Result<Self, sysinfo::Error> {
        let mut sys = System::new()?;
        sys.refresh_cpu_usage();
        Ok(Self {
            state: Mutex::new(SystemState {
                sys,
                disks: Disks::new()?,
                networks: Networks::new()?,
                components: Components::new()?,
                cores: System::physical_core_count().ok(),
                process_count: 0,
                inventory_refreshed: None,
                sensors_rescanned: None,
            }),
            hostname: whoami::fallible::hostname().unwrap_or_else(|_| "unknown".to_string()),
            device_name: whoami::fallible::devicename()
                .or_else(|_| whoami::fallible::hostname())
                .unwrap_or_else(|_| "unknown".to_string()),
        })
    }

    /// Collect a full system snapshot.
    pub fn collect_system_snapshot(&self) -> SystemSnapshot {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let SystemState {
            sys,
            disks: disk_list,
            networks: net_list,
            components,
            cores,
            process_count,
            inventory_refreshed,
            sensors_rescanned,
        } = &mut *state;
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        // Slow inventory/sensors need not be rescanned on every fast CPU/network tick.
        // The default exporter interval is five seconds; faster UIs reuse this inventory.
        if inventory_refreshed.is_none_or(|t| t.elapsed() >= INVENTORY_INTERVAL) {
            *process_count = count_processes(sys);
            disk_list.refresh(true);
            if sensors_rescanned.is_none_or(|t| t.elapsed() >= SENSOR_RESCAN_INTERVAL) {
                components.refresh(true);
                *sensors_rescanned = Some(Instant::now());
            } else {
                components.iter_mut().for_each(|c| c.refresh());
            }
            *inventory_refreshed = Some(Instant::now());
        }

        let cpu_usage = sys.global_cpu_usage() as f64;
        let cpu_core_count = *cores;
        let memory_total = sys.total_memory();
        let memory_used = sys.used_memory();
        let memory_available = sys.available_memory();
        let swap_total = sys.total_swap();
        let swap_used = sys.used_swap();
        let uptime = System::uptime().unwrap_or_default();
        let load = System::load_average().unwrap_or(sysinfo::LoadAvg {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        });
        let process_count = *process_count;

        // Disks
        let disks: Vec<DiskInfo> = disk_list
            .iter()
            .map(|d| DiskInfo {
                name: d.name().to_string_lossy().to_string(),
                mount_point: d.mount_point().to_string_lossy().to_string(),
                file_system: d.file_system().to_string_lossy().to_string(),
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
                is_removable: d.is_removable(),
            })
            .collect();

        // Networks
        net_list.refresh(true);
        let networks: Vec<NetworkInfo> = net_list
            .iter()
            .map(|(name, data)| NetworkInfo {
                interface: name.to_string_lossy().into_owned(),
                mac_address: data.mac_address().to_string(),
                received_bytes: data.received(),
                transmitted_bytes: data.transmitted(),
                total_received_bytes: data.total_received(),
                total_transmitted_bytes: data.total_transmitted(),
            })
            .collect();

        // Temperatures
        let temperatures: Vec<TemperatureInfo> = components
            .iter()
            .filter_map(|c| {
                let temp = c.temperature()?;
                Some(TemperatureInfo {
                    label: c.label().to_string(),
                    current_celsius: temp as f64,
                    max_celsius: c.max().map(|m| m as f64),
                })
            })
            .collect();

        SystemSnapshot {
            hostname: self.hostname.clone(),
            device_name: self.device_name.clone(),
            cpu_usage_percent: cpu_usage,
            cpu_core_count,
            memory_total_bytes: memory_total,
            memory_used_bytes: memory_used,
            memory_available_bytes: memory_available,
            swap_total_bytes: swap_total,
            swap_used_bytes: swap_used,
            uptime_seconds: uptime,
            load_avg_1m: load.one,
            load_avg_5m: load.five,
            load_avg_15m: load.fifteen,
            process_count,
            disks,
            networks,
            temperatures,
        }
    }
}

#[async_trait]
impl Collector for SystemCollector {
    fn name(&self) -> &'static str {
        "system"
    }

    async fn init(&mut self) -> Result<usize> {
        // System collector always has exactly 1 "device" (the host)
        debug!("System collector initialized");
        Ok(1)
    }

    async fn collect(&self) -> Result<Vec<GpuSnapshot>> {
        // System collector doesn't produce GpuSnapshots — it uses its own path.
        // We return an empty vec here; the system metrics are exported separately.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_count_is_cached_between_inventory_refreshes() {
        let collector = SystemCollector::new().unwrap();
        let first = collector.collect_system_snapshot();
        // This test process itself is always counted.
        assert!(first.process_count >= 1);
        let state = collector.state.lock().unwrap();
        assert_eq!(state.process_count, first.process_count);
        assert!(state.inventory_refreshed.is_some() && state.sensors_rescanned.is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_process_count_reads_proc_without_a_sysinfo_scan() {
        let mut sys = System::new().unwrap();
        assert!(count_processes(&mut sys) >= 1);
        // The cheap path never populates sysinfo's process table.
        assert!(sys.processes().is_empty());
    }
}

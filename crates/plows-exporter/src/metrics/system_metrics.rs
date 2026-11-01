//! System-level Prometheus metrics (CPU, memory, disk, network, temperature).

use once_cell::sync::Lazy;
use prometheus::{GaugeVec, Opts};

use super::registry::REGISTRY;

/// Helper to create and register a GaugeVec.
fn sys_gauge(name: &str, help: &str, extra_labels: &[&str]) -> GaugeVec {
    let mut labels = vec!["hostname", "device_name"];
    labels.extend_from_slice(extra_labels);
    let gauge = GaugeVec::new(Opts::new(name, help), &labels)
        .unwrap_or_else(|e| panic!("Failed to create metric '{name}': {e}"));
    REGISTRY
        .register(Box::new(gauge.clone()))
        .unwrap_or_else(|e| panic!("Failed to register metric '{name}': {e}"));
    gauge
}

/// All system-level Prometheus metrics.
pub struct SystemMetrics {
    // CPU
    pub cpu_usage_percent: GaugeVec,
    pub cpu_core_count: GaugeVec,

    // Memory
    pub memory_total_bytes: GaugeVec,
    pub memory_used_bytes: GaugeVec,
    pub memory_available_bytes: GaugeVec,
    pub swap_total_bytes: GaugeVec,
    pub swap_used_bytes: GaugeVec,

    // System
    pub uptime_seconds: GaugeVec,
    pub process_count: GaugeVec,
    pub load_average: GaugeVec,

    // Disk
    pub disk_total_bytes: GaugeVec,
    pub disk_available_bytes: GaugeVec,

    // Network
    pub network_received_bytes: GaugeVec,
    pub network_transmitted_bytes: GaugeVec,
    pub network_total_received_bytes: GaugeVec,
    pub network_total_transmitted_bytes: GaugeVec,

    // Temperature
    pub component_temperature_celsius: GaugeVec,
    pub component_temperature_max_celsius: GaugeVec,
}

impl SystemMetrics {
    fn new() -> Self {
        Self {
            cpu_usage_percent: sys_gauge(
                "system_cpu_usage_percent",
                "Global CPU usage percentage",
                &[],
            ),
            cpu_core_count: sys_gauge("system_cpu_core_count", "Number of physical CPU cores", &[]),
            memory_total_bytes: sys_gauge(
                "system_memory_total_bytes",
                "Total system memory in bytes",
                &[],
            ),
            memory_used_bytes: sys_gauge(
                "system_memory_used_bytes",
                "Used system memory in bytes",
                &[],
            ),
            memory_available_bytes: sys_gauge(
                "system_memory_available_bytes",
                "Available system memory in bytes",
                &[],
            ),
            swap_total_bytes: sys_gauge(
                "system_swap_total_bytes",
                "Total swap space in bytes",
                &[],
            ),
            swap_used_bytes: sys_gauge("system_swap_used_bytes", "Used swap space in bytes", &[]),
            uptime_seconds: sys_gauge("system_uptime_seconds", "System uptime in seconds", &[]),
            process_count: sys_gauge("system_process_count", "Number of running processes", &[]),
            load_average: sys_gauge("system_load_average", "System load average", &["duration"]),
            disk_total_bytes: sys_gauge(
                "system_disk_total_bytes",
                "Total disk space in bytes",
                &["disk_name", "mount_point", "file_system", "is_removable"],
            ),
            disk_available_bytes: sys_gauge(
                "system_disk_available_bytes",
                "Available disk space in bytes",
                &["disk_name", "mount_point", "file_system", "is_removable"],
            ),
            network_received_bytes: sys_gauge(
                "system_network_received_bytes",
                "Recently received bytes on network interface",
                &["interface", "mac_address"],
            ),
            network_transmitted_bytes: sys_gauge(
                "system_network_transmitted_bytes",
                "Recently transmitted bytes on network interface",
                &["interface", "mac_address"],
            ),
            network_total_received_bytes: sys_gauge(
                "system_network_total_received_bytes",
                "Total received bytes on network interface",
                &["interface", "mac_address"],
            ),
            network_total_transmitted_bytes: sys_gauge(
                "system_network_total_transmitted_bytes",
                "Total transmitted bytes on network interface",
                &["interface", "mac_address"],
            ),
            component_temperature_celsius: sys_gauge(
                "system_component_temperature_celsius",
                "Current temperature of hardware component",
                &["component"],
            ),
            component_temperature_max_celsius: sys_gauge(
                "system_component_temperature_max_celsius",
                "Maximum recorded temperature of hardware component",
                &["component"],
            ),
        }
    }
}

/// Global system metrics instance.
pub static SYSTEM_METRICS: Lazy<SystemMetrics> = Lazy::new(SystemMetrics::new);

/// Update system Prometheus gauges from a system snapshot.
#[cfg(feature = "system")]
pub fn update_system_metrics(snapshot: &crate::collector::system::SystemSnapshot) {
    let base_labels = &[snapshot.hostname.as_str(), snapshot.device_name.as_str()];
    let mut previous = PREVIOUS.lock().unwrap_or_else(|e| e.into_inner());
    let base = || vec![snapshot.hostname.clone(), snapshot.device_name.clone()];
    let disks = snapshot
        .disks
        .iter()
        .map(|d| {
            let mut labels = base();
            labels.extend([
                d.name.clone(),
                d.mount_point.clone(),
                d.file_system.clone(),
                d.is_removable.to_string(),
            ]);
            labels
        })
        .collect();
    prune(
        &mut previous[0],
        disks,
        &[
            &SYSTEM_METRICS.disk_total_bytes,
            &SYSTEM_METRICS.disk_available_bytes,
        ],
    );
    let networks = snapshot
        .networks
        .iter()
        .map(|n| {
            let mut labels = base();
            labels.extend([n.interface.clone(), n.mac_address.clone()]);
            labels
        })
        .collect();
    prune(
        &mut previous[1],
        networks,
        &[
            &SYSTEM_METRICS.network_received_bytes,
            &SYSTEM_METRICS.network_transmitted_bytes,
            &SYSTEM_METRICS.network_total_received_bytes,
            &SYSTEM_METRICS.network_total_transmitted_bytes,
        ],
    );
    let temperatures = snapshot
        .temperatures
        .iter()
        .map(|t| {
            let mut labels = base();
            labels.push(t.label.clone());
            labels
        })
        .collect();
    prune(
        &mut previous[2],
        temperatures,
        &[
            &SYSTEM_METRICS.component_temperature_celsius,
            &SYSTEM_METRICS.component_temperature_max_celsius,
        ],
    );

    SYSTEM_METRICS
        .cpu_usage_percent
        .with_label_values(base_labels)
        .set(snapshot.cpu_usage_percent);

    if let Some(cores) = snapshot.cpu_core_count {
        SYSTEM_METRICS
            .cpu_core_count
            .with_label_values(base_labels)
            .set(cores as f64);
    } else {
        let _ = SYSTEM_METRICS
            .cpu_core_count
            .remove_label_values(base_labels);
    }

    SYSTEM_METRICS
        .memory_total_bytes
        .with_label_values(base_labels)
        .set(snapshot.memory_total_bytes as f64);

    SYSTEM_METRICS
        .memory_used_bytes
        .with_label_values(base_labels)
        .set(snapshot.memory_used_bytes as f64);

    SYSTEM_METRICS
        .memory_available_bytes
        .with_label_values(base_labels)
        .set(snapshot.memory_available_bytes as f64);

    SYSTEM_METRICS
        .swap_total_bytes
        .with_label_values(base_labels)
        .set(snapshot.swap_total_bytes as f64);

    SYSTEM_METRICS
        .swap_used_bytes
        .with_label_values(base_labels)
        .set(snapshot.swap_used_bytes as f64);

    SYSTEM_METRICS
        .uptime_seconds
        .with_label_values(base_labels)
        .set(snapshot.uptime_seconds as f64);

    SYSTEM_METRICS
        .process_count
        .with_label_values(base_labels)
        .set(snapshot.process_count as f64);

    // Load averages
    SYSTEM_METRICS
        .load_average
        .with_label_values(&[base_labels[0], base_labels[1], "1m"])
        .set(snapshot.load_avg_1m);
    SYSTEM_METRICS
        .load_average
        .with_label_values(&[base_labels[0], base_labels[1], "5m"])
        .set(snapshot.load_avg_5m);
    SYSTEM_METRICS
        .load_average
        .with_label_values(&[base_labels[0], base_labels[1], "15m"])
        .set(snapshot.load_avg_15m);

    // Disks
    for disk in &snapshot.disks {
        let disk_labels = &[
            base_labels[0],
            base_labels[1],
            &disk.name,
            &disk.mount_point,
            &disk.file_system,
            if disk.is_removable { "true" } else { "false" },
        ];
        SYSTEM_METRICS
            .disk_total_bytes
            .with_label_values(disk_labels)
            .set(disk.total_bytes as f64);
        SYSTEM_METRICS
            .disk_available_bytes
            .with_label_values(disk_labels)
            .set(disk.available_bytes as f64);
    }

    // Networks
    for net in &snapshot.networks {
        let net_labels = &[
            base_labels[0],
            base_labels[1],
            &net.interface,
            &net.mac_address,
        ];
        SYSTEM_METRICS
            .network_received_bytes
            .with_label_values(net_labels)
            .set(net.received_bytes as f64);
        SYSTEM_METRICS
            .network_transmitted_bytes
            .with_label_values(net_labels)
            .set(net.transmitted_bytes as f64);
        SYSTEM_METRICS
            .network_total_received_bytes
            .with_label_values(net_labels)
            .set(net.total_received_bytes as f64);
        SYSTEM_METRICS
            .network_total_transmitted_bytes
            .with_label_values(net_labels)
            .set(net.total_transmitted_bytes as f64);
    }

    // Temperatures
    for temp in &snapshot.temperatures {
        let temp_labels = &[base_labels[0], base_labels[1], &temp.label];
        SYSTEM_METRICS
            .component_temperature_celsius
            .with_label_values(temp_labels)
            .set(temp.current_celsius);
        if let Some(max) = temp.max_celsius {
            SYSTEM_METRICS
                .component_temperature_max_celsius
                .with_label_values(temp_labels)
                .set(max);
        } else {
            let _ = SYSTEM_METRICS
                .component_temperature_max_celsius
                .remove_label_values(temp_labels);
        }
    }
}

#[cfg(feature = "system")]
type LabelSet = std::collections::HashSet<Vec<String>>;
#[cfg(feature = "system")]
static PREVIOUS: std::sync::LazyLock<std::sync::Mutex<[LabelSet; 3]>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::array::from_fn(|_| LabelSet::new())));

#[cfg(feature = "system")]
fn prune(previous: &mut LabelSet, current: LabelSet, gauges: &[&GaugeVec]) {
    for old in previous.difference(&current) {
        let labels: Vec<&str> = old.iter().map(String::as_str).collect();
        for gauge in gauges {
            let _ = gauge.remove_label_values(&labels);
        }
    }
    *previous = current;
}

#[cfg(all(test, feature = "system"))]
mod tests {
    use super::*;
    #[test]
    fn removed_devices_drop_their_series() {
        use prometheus::core::Collector;
        let gauge = GaugeVec::new(Opts::new("test_disks", "test"), &["disk"]).unwrap();
        gauge.with_label_values(&["old"]).set(10.0);
        gauge.with_label_values(&["new"]).set(20.0);
        let mut previous = [vec!["old".into()]].into();
        prune(&mut previous, [vec!["new".into()]].into(), &[&gauge]);
        assert_eq!(gauge.collect()[0].get_metric().len(), 1);
    }
}

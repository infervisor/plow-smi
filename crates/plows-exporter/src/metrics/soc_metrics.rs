//! Optional SoC metrics, independent of dedicated GPU memory and vendor SDKs.

use super::registry::REGISTRY;
use once_cell::sync::Lazy;
use plows_gpu::{EngineMetrics, SocMetrics};
use prometheus::{GaugeVec, Opts};
use std::sync::Mutex;

const LABELS: &[&str] = &["gpu_index", "vendor", "hostname", "brand", "uuid"];

fn gauge(name: &str, help: &str, extra: &[&str]) -> GaugeVec {
    let labels: Vec<_> = LABELS.iter().chain(extra).copied().collect();
    let metric = GaugeVec::new(Opts::new(name, help), &labels).expect("valid SoC metric");
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("unique SoC metric");
    metric
}

struct EngineGauges {
    utilization: GaugeVec,
    power: GaugeVec,
    clock: GaugeVec,
    temperature: GaugeVec,
    cores: GaugeVec,
}
impl EngineGauges {
    fn new(prefix: &str, labels: &[&str]) -> Self {
        Self {
            utilization: gauge(
                &format!("{prefix}_utilization_percent"),
                "Engine active residency over the sample window (0-100); absent when unsupported",
                labels,
            ),
            power: gauge(
                &format!("{prefix}_power_watts"),
                "Average engine power over the sample window in watts",
                labels,
            ),
            clock: gauge(
                &format!("{prefix}_clock_mhz"),
                "Engine active-weighted clock frequency in MHz",
                labels,
            ),
            temperature: gauge(
                &format!("{prefix}_temperature_celsius"),
                "Average engine sensor temperature in Celsius",
                labels,
            ),
            cores: gauge(
                &format!("{prefix}_cores_total"),
                "Physical engine core count",
                labels,
            ),
        }
    }
    fn remove(&self, labels: &[&str]) {
        for metric in [
            &self.utilization,
            &self.power,
            &self.clock,
            &self.temperature,
            &self.cores,
        ] {
            let _ = metric.remove_label_values(labels);
        }
    }
    fn update(&self, labels: &[&str], engine: &EngineMetrics) {
        for (metric, value) in [
            (&self.utilization, engine.utilization.map(f64::from)),
            (&self.power, engine.power_watts.map(f64::from)),
            (&self.clock, engine.clock_mhz.map(f64::from)),
            (&self.temperature, engine.temperature_celsius.map(f64::from)),
            (&self.cores, engine.cores.map(f64::from)),
        ] {
            if let Some(value) = value.filter(|v| v.is_finite()) {
                metric.with_label_values(labels).set(value);
            } else {
                let _ = metric.remove_label_values(labels);
            }
        }
    }
}

struct SocGauges {
    cpu: EngineGauges,
    npu: EngineGauges,
    clusters: EngineGauges,
    memory_used: GaugeVec,
    memory_total: GaugeVec,
    compute_power: GaugeVec,
    system_power: GaugeVec,
    temperatures: GaugeVec,
}
impl SocGauges {
    fn new() -> Self {
        Self {
            cpu: EngineGauges::new("apple_cpu", &[]),
            npu: EngineGauges::new("apple_npu", &[]),
            clusters: EngineGauges::new("apple_cpu_cluster", &["cluster"]),
            memory_used: gauge(
                "apple_unified_memory_used_bytes",
                "System-wide used unified memory, not GPU allocations",
                &[],
            ),
            memory_total: gauge(
                "apple_unified_memory_total_bytes",
                "Installed unified physical memory shared by all SoC engines",
                &[],
            ),
            compute_power: gauge(
                "apple_compute_power_watts",
                "Sum of CPU, GPU and Neural Engine power; not whole-system power",
                &[],
            ),
            system_power: gauge(
                "apple_system_power_watts",
                "Whole-system power reported by SMC PSTR when available",
                &[],
            ),
            temperatures: gauge(
                "apple_temperature_celsius",
                "Named SMC or IOHID temperature sensors",
                &["sensor"],
            ),
        }
    }
    fn remove(&self, labels: &[&str], soc: &SocMetrics) {
        self.cpu.remove(labels);
        self.npu.remove(labels);
        for metric in [
            &self.memory_used,
            &self.memory_total,
            &self.compute_power,
            &self.system_power,
        ] {
            let _ = metric.remove_label_values(labels);
        }
        self.remove_children(labels, soc, None);
    }
    fn remove_children(&self, labels: &[&str], old: &SocMetrics, new: Option<&SocMetrics>) {
        for cluster in &old.cpu_clusters {
            if new.is_none_or(|s| !s.cpu_clusters.iter().any(|c| c.name == cluster.name)) {
                let labels = [
                    labels[0],
                    labels[1],
                    labels[2],
                    labels[3],
                    labels[4],
                    &cluster.name,
                ];
                self.clusters.remove(&labels);
            }
        }
        for sensor in &old.temperatures {
            if new.is_none_or(|s| {
                !s.temperatures
                    .iter()
                    .any(|t| t.name == sensor.name && t.celsius.is_finite())
            }) {
                let labels = [
                    labels[0],
                    labels[1],
                    labels[2],
                    labels[3],
                    labels[4],
                    &sensor.name,
                ];
                let _ = self.temperatures.remove_label_values(&labels);
            }
        }
    }
    fn update(&self, labels: &[&str], soc: &SocMetrics) {
        self.cpu.update(labels, &soc.cpu);
        self.npu.update(labels, &soc.npu);
        for cluster in &soc.cpu_clusters {
            let labels = [
                labels[0],
                labels[1],
                labels[2],
                labels[3],
                labels[4],
                &cluster.name,
            ];
            self.clusters.update(&labels, &cluster.metrics);
        }
        for (metric, value) in [
            (&self.memory_used, soc.memory_used_bytes.map(|v| v as f64)),
            (&self.memory_total, soc.memory_total_bytes.map(|v| v as f64)),
            (&self.compute_power, soc.compute_power_watts.map(f64::from)),
            (&self.system_power, soc.system_power_watts.map(f64::from)),
        ] {
            if let Some(value) = value.filter(|v| v.is_finite()) {
                metric.with_label_values(labels).set(value);
            } else {
                let _ = metric.remove_label_values(labels);
            }
        }
        for sensor in &soc.temperatures {
            if sensor.celsius.is_finite() {
                let labels = [
                    labels[0],
                    labels[1],
                    labels[2],
                    labels[3],
                    labels[4],
                    &sensor.name,
                ];
                self.temperatures
                    .with_label_values(&labels)
                    .set(sensor.celsius as f64);
            }
        }
    }
}
static SOC_METRICS: Lazy<SocGauges> = Lazy::new(SocGauges::new);
static PREVIOUS: Mutex<Vec<crate::collector::GpuSnapshot>> = Mutex::new(Vec::new());

/// Publish complete optional SoC snapshots; remove samples that became unavailable.
pub fn update_soc_metrics(snapshots: &[crate::collector::GpuSnapshot]) {
    let mut previous = PREVIOUS.lock().unwrap_or_else(|e| e.into_inner());
    for old in previous.iter() {
        let index = old.index.to_string();
        let labels = [&*index, old.vendor, &old.hostname, &old.brand, &old.uuid];
        let new = snapshots
            .iter()
            .find(|s| {
                s.index == old.index
                    && s.vendor == old.vendor
                    && s.hostname == old.hostname
                    && s.brand == old.brand
                    && s.uuid == old.uuid
            })
            .and_then(|s| s.soc.as_ref());
        if let Some(soc) = &old.soc {
            if new.is_none() {
                SOC_METRICS.remove(&labels, soc);
            } else {
                SOC_METRICS.remove_children(&labels, soc, new);
            }
        }
    }
    for snap in snapshots.iter().filter(|s| s.vendor == "apple") {
        if let Some(soc) = &snap.soc {
            let index = snap.index.to_string();
            SOC_METRICS.update(
                &[&index, snap.vendor, &snap.hostname, &snap.brand, &snap.uuid],
                soc,
            );
        }
    }
    previous.clear();
    previous.extend(
        snapshots
            .iter()
            .filter(|s| s.vendor == "apple" && s.soc.is_some())
            .cloned(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_metrics_preserve_fractional_values_and_remove_stale_series() {
        let mut snap = crate::collector::GpuSnapshot::new(
            0,
            "apple",
            "test".into(),
            "Apple test".into(),
            "test-soc".into(),
        );
        snap.soc = Some(SocMetrics {
            npu: EngineMetrics {
                power_watts: Some(0.125),
                ..Default::default()
            },
            ..Default::default()
        });
        update_soc_metrics(&[snap]);
        let families = REGISTRY.gather();
        let npu = families
            .iter()
            .find(|f| f.name() == "apple_npu_power_watts")
            .unwrap();
        assert_eq!(npu.get_metric()[0].get_gauge().get_value(), 0.125);
        assert!(!families
            .iter()
            .any(|f| f.name() == "apple_npu_utilization_percent"));
        // Missing fields on a still-present device must remove the previous value.
        let mut missing = crate::collector::GpuSnapshot::new(
            0,
            "apple",
            "test".into(),
            "Apple test".into(),
            "test-soc".into(),
        );
        missing.soc = Some(SocMetrics::default());
        update_soc_metrics(&[missing]);
        assert!(!REGISTRY
            .gather()
            .iter()
            .any(|f| f.name() == "apple_npu_power_watts"));
        update_soc_metrics(&[]);
        assert!(!REGISTRY
            .gather()
            .iter()
            .any(|f| f.name() == "apple_npu_power_watts"));
    }
}

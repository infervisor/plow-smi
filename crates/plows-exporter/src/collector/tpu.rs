//! Google Cloud TPU collector.
//!
//! Collects metrics from Google Cloud TPUs via:
//! 1. The local TPU runtime metrics endpoint (http://localhost:8431/metrics)
//! 2. GCE instance metadata API for TPU identity information
//!
//! This collector works on TPU VMs (v2, v3, v4, v5e, v5p) where the TPU
//! runtime exposes Prometheus-compatible metrics locally.

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::collector::{Collector, GpuSnapshot};
use crate::error::{ExporterError, Result};

/// Default TPU runtime metrics endpoint.
const TPU_METRICS_URL: &str = "http://localhost:8431/metrics";

/// GCE metadata endpoint for TPU info.
const GCE_METADATA_URL: &str = "http://metadata.google.internal/computeMetadata/v1";

/// Google Cloud TPU metrics collector.
pub struct TpuCollector {
    hostname: String,
    tpu_type: String,
    tpu_name: String,
    chip_count: u32,
    metrics_url: String,
    client: reqwest::Client,
}

/// Parsed TPU metrics from the runtime endpoint.
#[derive(Debug, Clone, Default)]
struct TpuDeviceMetrics {
    /// Duty cycle percentage (0-100) — equivalent to GPU utilization.
    pub duty_cycle_percent: Option<f64>,
    /// HBM (High Bandwidth Memory) used in bytes.
    pub hbm_used_bytes: Option<u64>,
    /// HBM total in bytes.
    pub hbm_total_bytes: Option<u64>,
    /// TensorCore utilization percentage.
    pub tensorcore_utilization: Option<f64>,
    /// Current power draw in watts.
    pub power_watts: Option<f64>,
    /// Temperature in Celsius.
    pub temperature_celsius: Option<f64>,
}

impl TpuCollector {
    /// Create a new TPU collector.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .connect_timeout(std::time::Duration::from_secs(2))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("valid TPU HTTP client configuration"),
            hostname: whoami::fallible::hostname().unwrap_or_else(|_| "unknown".to_string()),
            tpu_type: String::new(),
            tpu_name: String::new(),
            chip_count: 0,
            metrics_url: TPU_METRICS_URL.to_string(),
        }
    }

    /// Create a TPU collector with a custom metrics endpoint.
    pub fn with_metrics_url(mut self, url: String) -> Self {
        self.metrics_url = url;
        self
    }

    /// Query GCE metadata API for TPU information.
    async fn fetch_tpu_metadata(&mut self) -> Result<()> {
        // Try to get TPU accelerator type from metadata
        let client = &self.client;

        // Get accelerator type (e.g., "v4-8", "v5e-4")
        let accel_type = client
            .get(format!(
                "{}/instance/attributes/accelerator-type",
                GCE_METADATA_URL
            ))
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .ok()
            .and_then(|r| {
                if r.status().is_success() {
                    Some(r)
                } else {
                    None
                }
            });

        if let Some(resp) = accel_type {
            if let Ok(text) = resp.text().await {
                self.tpu_type = text.trim().to_string();
            }
        }

        // Get instance name
        let instance_name = client
            .get(format!("{}/instance/name", GCE_METADATA_URL))
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .ok()
            .and_then(|r| {
                if r.status().is_success() {
                    Some(r)
                } else {
                    None
                }
            });

        if let Some(resp) = instance_name {
            if let Ok(text) = resp.text().await {
                self.tpu_name = text.trim().to_string();
            }
        }

        // Parse chip count from accelerator type (e.g., "v4-8" -> 8 chips)
        if let Some(count_str) = self.tpu_type.split('-').next_back() {
            self.chip_count = count_str.parse::<u32>().unwrap_or(1);
        }

        if self.tpu_type.is_empty() {
            self.tpu_type = "unknown-tpu".to_string();
        }
        if self.tpu_name.is_empty() {
            self.tpu_name = self.hostname.clone();
        }

        Ok(())
    }

    /// Fetch and parse TPU runtime metrics from the local endpoint.
    async fn fetch_runtime_metrics(&self) -> Result<Vec<(u32, TpuDeviceMetrics)>> {
        let client = &self.client;
        let response =
            client.get(&self.metrics_url).send().await.map_err(|e| {
                ExporterError::System(format!("TPU metrics endpoint unreachable: {e}"))
            })?;

        if !response.status().is_success() {
            return Err(ExporterError::System(format!(
                "TPU metrics endpoint returned status {}",
                response.status()
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| ExporterError::System(format!("Failed to read TPU metrics: {e}")))?;

        Ok(self.parse_tpu_metrics(&body))
    }

    /// Parse Prometheus text format metrics from TPU runtime.
    /// The TPU runtime exposes metrics like:
    ///   tpu_chip_duty_cycle{chip="0"} 85.2
    ///   tpu_chip_hbm_memory_used_bytes{chip="0"} 12345678
    ///   tpu_chip_hbm_memory_total_bytes{chip="0"} 34359738368
    ///   tpu_chip_tensorcore_utilization{chip="0"} 0.75
    ///   tpu_chip_power_watts{chip="0"} 120.5
    ///   tpu_chip_temperature_celsius{chip="0"} 45.0
    fn parse_tpu_metrics(&self, body: &str) -> Vec<(u32, TpuDeviceMetrics)> {
        let mut device_map: std::collections::HashMap<u32, TpuDeviceMetrics> =
            std::collections::HashMap::new();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse chip index from labels like {chip="0"} or {chip_id="0"}
            let chip_id = Self::extract_chip_id(line);
            let Some(chip_id) = chip_id else { continue };

            // Unrelated runtime metrics must not manufacture empty TPU devices.
            let name = line.split_once('{').map(|(name, _)| name).unwrap_or("");
            if ![
                "duty_cycle",
                "hbm_memory_used",
                "memory_used_bytes",
                "hbm_memory_total",
                "memory_total_bytes",
                "tensorcore_utilization",
                "power_watts",
                "power_draw",
                "temperature",
            ]
            .iter()
            .any(|known| name.contains(known))
                || Self::extract_value(line).is_none()
            {
                continue;
            }
            let metrics = device_map.entry(chip_id).or_default();

            // Parse known metric names
            if line.starts_with("tpu_chip_duty_cycle") || line.contains("duty_cycle") {
                if let Some(val) = Self::extract_value(line) {
                    metrics.duty_cycle_percent = Some(val);
                }
            } else if line.contains("hbm_memory_used") || line.contains("memory_used_bytes") {
                if let Some(val) = Self::extract_value(line) {
                    metrics.hbm_used_bytes = Some(val as u64);
                }
            } else if line.contains("hbm_memory_total") || line.contains("memory_total_bytes") {
                if let Some(val) = Self::extract_value(line) {
                    metrics.hbm_total_bytes = Some(val as u64);
                }
            } else if line.contains("tensorcore_utilization") {
                if let Some(val) = Self::extract_value(line) {
                    // Might be 0.0-1.0 or 0-100
                    let percent = if val <= 1.0 { val * 100.0 } else { val };
                    metrics.tensorcore_utilization = Some(percent);
                }
            } else if line.contains("power_watts") || line.contains("power_draw") {
                if let Some(val) = Self::extract_value(line) {
                    metrics.power_watts = Some(val);
                }
            } else if line.contains("temperature") {
                if let Some(val) = Self::extract_value(line) {
                    metrics.temperature_celsius = Some(val);
                }
            }
        }

        let mut result: Vec<(u32, TpuDeviceMetrics)> = device_map.into_iter().collect();
        result.sort_by_key(|(id, _)| *id);
        result
    }

    /// Extract chip ID from a Prometheus metric line.
    fn extract_chip_id(line: &str) -> Option<u32> {
        let labels = line.split_once('{')?.1.split_once('}')?.0;
        labels.split(',').find_map(|label| {
            let (name, value) = label.trim().split_once('=')?;
            matches!(name.trim(), "chip" | "chip_id")
                .then(|| value.trim().trim_matches('"').parse().ok())
                .flatten()
        })
    }

    /// Extract the numeric value from a Prometheus metric line.
    fn extract_value(line: &str) -> Option<f64> {
        // An optional timestamp follows the value; never parse it as telemetry.
        let rest = line.split_once('}')?.1;
        rest.split_whitespace()
            .next()?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
    }
}

#[async_trait]
impl Collector for TpuCollector {
    fn name(&self) -> &'static str {
        "tpu"
    }

    async fn init(&mut self) -> Result<usize> {
        // Try to detect TPU via metadata API
        info!("Detecting Google Cloud TPU...");

        if let Err(e) = self.fetch_tpu_metadata().await {
            warn!(error = %e, "Failed to fetch TPU metadata (not on GCE TPU VM?)");
        }

        // Verify the TPU runtime metrics endpoint is reachable
        let client = &self.client;

        match client.get(&self.metrics_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body = resp.text().await.unwrap_or_default();
                // Count unique chip IDs
                let chip_metrics = self.parse_tpu_metrics(&body);
                if chip_metrics.is_empty() {
                    // Might be a valid endpoint but no chip metrics yet
                    if self.chip_count == 0 {
                        self.chip_count = 1;
                    }
                } else {
                    self.chip_count = chip_metrics.len() as u32;
                }
                info!(
                    tpu_type = %self.tpu_type,
                    tpu_name = %self.tpu_name,
                    chips = self.chip_count,
                    "TPU detected"
                );
                Ok(self.chip_count as usize)
            }
            Ok(resp) => Err(ExporterError::System(format!(
                "TPU metrics endpoint returned status: {}",
                resp.status()
            ))),
            Err(e) => Err(ExporterError::System(format!(
                "TPU runtime metrics not available at {}: {e}",
                self.metrics_url
            ))),
        }
    }

    async fn collect(&self) -> Result<Vec<GpuSnapshot>> {
        let chip_metrics = self.fetch_runtime_metrics().await?;
        let mut snapshots = Vec::with_capacity(chip_metrics.len());

        for (chip_id, metrics) in &chip_metrics {
            let uuid = format!("tpu-{}-chip-{}", self.tpu_name, chip_id);
            let brand = format!("Google TPU {}", self.tpu_type);

            let mut snapshot =
                GpuSnapshot::new(*chip_id, "tpu", self.hostname.clone(), brand, uuid);

            // Map TPU metrics to GpuSnapshot fields
            if let Some(duty) = metrics
                .duty_cycle_percent
                .or(metrics.tensorcore_utilization)
            {
                snapshot.gpu_utilization_percent = Some(duty as i64);
            }

            if let Some(hbm_total) = metrics.hbm_total_bytes {
                snapshot.memory_total_bytes = Some(hbm_total);
            }
            if let Some(hbm_used) = metrics.hbm_used_bytes {
                snapshot.memory_used_bytes = Some(hbm_used);
            }
            if let (Some(total), Some(used)) = (metrics.hbm_total_bytes, metrics.hbm_used_bytes) {
                snapshot.memory_free_bytes = Some(total.saturating_sub(used));
                if total > 0 {
                    snapshot.memory_utilization_percent =
                        Some(((used as f64 / total as f64) * 100.0) as i64);
                }
            }

            if let Some(power) = metrics.power_watts {
                snapshot.power_usage_mw = Some((power * 1000.0) as u64);
            }

            if let Some(temp) = metrics.temperature_celsius {
                snapshot.temperature_celsius = Some(temp as i64);
            }

            debug!(chip_id, vendor = "tpu", "Collected TPU metrics");
            snapshots.push(snapshot);
        }

        Ok(snapshots)
    }
}

impl Default for TpuCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_and_timestamps_are_parsed_without_confusion() {
        let line = "tpu_chip_power_watts{host=\"x\",chip_id=\"2\"} 42.5 1700000000";
        assert_eq!(TpuCollector::extract_chip_id(line), Some(2));
        assert_eq!(TpuCollector::extract_value(line), Some(42.5));
        assert_eq!(TpuCollector::extract_value("m{chip=\"0\"} NaN"), None);
        assert_eq!(TpuCollector::extract_value("m{chip=\"0\"} -1"), None);
        assert_eq!(
            TpuCollector::extract_chip_id("tpu_chip_power{host=\"2\"} 1"),
            None
        );
    }
}

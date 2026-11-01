//! Apple Silicon backend: thread-confined native handles and non-blocking snapshots.

use crate::{DeviceMetrics, GpuBackend, GpuDevice, GpuError, Result, SocMetrics, Vendor};

/// Read-only Apple Silicon GPU/SoC backend.
///
/// Native handles stay on one sampler thread; consumers only copy owned snapshots.
/// No subprocesses, privilege escalation, or sleeps occur in `refresh()`.
pub struct AppleBackend {
    identity: GpuDevice,
    metrics: std::sync::Arc<DeviceMetrics>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    worker: Worker,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
type PublishedSample = Option<(std::time::Instant, std::sync::Arc<DeviceMetrics>)>;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct Worker {
    latest: std::sync::Arc<std::sync::Mutex<PublishedSample>>,
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AppleBackend {
    /// Discover Apple Silicon and start a one-second native sampler.
    pub fn try_load() -> Result<Self> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::sync::{mpsc, Arc, Mutex};
            use std::time::{Duration, Instant};
            let latest = Arc::new(Mutex::new(None));
            let shared = Arc::clone(&latest);
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let (stop, stopping) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("plows-apple-sampler".into())
                .spawn(move || {
                    let mut sampler = match crate::ffi::apple::AppleSampler::new() {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(sampler.identity().clone()));
                    loop {
                        let started = Instant::now();
                        let metrics = sampler.sample();
                        if let Ok(mut slot) = shared.lock() {
                            *slot = Some((Instant::now(), Arc::new(metrics)));
                        }
                        // Interruptible wait; shutdown never waits for a full sampling interval.
                        let remaining = Duration::from_secs(1).saturating_sub(started.elapsed());
                        if !matches!(
                            stopping.recv_timeout(remaining),
                            Err(mpsc::RecvTimeoutError::Timeout)
                        ) {
                            break;
                        }
                    }
                })
                .map_err(|e| GpuError::InitializationFailed(e.to_string()))?;
            let worker = Worker {
                latest,
                stop,
                thread: Some(thread),
            };
            let identity = ready_rx
                .recv()
                .map_err(|e| GpuError::InitializationFailed(e.to_string()))??;
            Ok(Self {
                identity,
                metrics: Arc::new(DeviceMetrics::default()),
                worker,
            })
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        Err(GpuError::Unsupported(
            "Apple Silicon requires arm64 macOS".into(),
        ))
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl GpuBackend for AppleBackend {
    fn vendor(&self) -> Vendor {
        Vendor::Apple
    }
    fn device_count(&self) -> usize {
        1
    }
    fn devices(&self) -> Vec<GpuDevice> {
        vec![self.identity.clone()]
    }
    fn refresh(&mut self) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let current = self.worker.latest.lock().ok().and_then(|slot| {
                slot.as_ref()
                    .filter(|(time, _)| time.elapsed() < std::time::Duration::from_secs(3))
                    .map(|(_, metrics)| std::sync::Arc::clone(metrics))
            });
            if let Some(current) = current {
                self.metrics = current;
            } else if self.metrics.soc.is_some() {
                self.metrics = std::sync::Arc::new(DeviceMetrics::default());
            }
        }
    }
    fn utilization(&self, id: usize) -> Option<f32> {
        (id == 0).then_some(self.metrics.utilization).flatten()
    }
    fn memory_used(&self, _id: usize) -> Option<u64> {
        None
    }
    fn memory_total(&self, _id: usize) -> Option<u64> {
        None
    }
    fn temperature(&self, id: usize) -> Option<f32> {
        (id == 0).then_some(self.metrics.temperature).flatten()
    }
    fn power_usage(&self, id: usize) -> Option<f32> {
        (id == 0).then_some(self.metrics.power_usage).flatten()
    }
    fn fan_speed(&self, _id: usize) -> Option<f32> {
        None
    }
    fn clock_graphics(&self, id: usize) -> Option<u32> {
        (id == 0).then_some(self.metrics.clock_graphics).flatten()
    }
    fn clock_memory(&self, _id: usize) -> Option<u32> {
        None
    }
    fn soc_metrics(&self, id: usize) -> Option<SocMetrics> {
        (id == 0).then(|| self.metrics.soc.clone()).flatten()
    }
}

/// Convert energy deltas to average watts. Unknown units/reset counters are absent.
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn energy_watts(value: i64, unit: &str, seconds: f64) -> Option<f32> {
    if value < 0 || !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let scale = match unit.trim() {
        "J" => 1.0,
        "mJ" => 1e-3,
        "uJ" | "µJ" => 1e-6,
        "nJ" => 1e-9,
        _ => return None,
    };
    let watts = value as f64 * scale / seconds;
    (watts.is_finite() && watts <= f32::MAX as f64).then_some(watts as f32)
}

/// Active residency and active-weighted frequency; mismatched tables omit only clocks.
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn residency(
    states: &[(String, i64)],
    frequencies: &[u32],
) -> (Option<f32>, Option<u32>) {
    if states.is_empty() || states.iter().any(|(_, time)| *time < 0) {
        return (None, None);
    }
    let total: f64 = states.iter().map(|(_, t)| *t as f64).sum();
    if total <= 0.0 {
        return (None, None);
    }
    let inactive = |name: &str| matches!(name, "IDLE" | "DOWN" | "OFF" | "SLEEP");
    let active = || states.iter().filter(|(name, _)| !inactive(name));
    let time: f64 = active().map(|(_, t)| *t as f64).sum();
    let clock = if time > 0.0 && active().count() == frequencies.len() && !frequencies.is_empty() {
        Some(
            (active()
                .zip(frequencies)
                .map(|((_, t), f)| *t as f64 * *f as f64)
                .sum::<f64>()
                / time)
                .round() as u32,
        )
    } else {
        None
    };
    (Some((100.0 * time / total) as f32), clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn energy_units_and_invalid_windows() {
        assert_eq!(energy_watts(1000, "mJ", 0.5), Some(2.0));
        assert_eq!(energy_watts(1_000_000, "uJ", 0.5), Some(2.0));
        assert_eq!(energy_watts(0, "nJ", 1.0), Some(0.0));
        for (v, u, t) in [
            (-1, "mJ", 1.0),
            (1, "watts", 1.0),
            (1, "mJ", 0.0),
            (1, "mJ", f64::NAN),
        ] {
            assert_eq!(energy_watts(v, u, t), None);
        }
    }
    #[test]
    fn residency_uses_idle_and_down_without_fabricating_clocks() {
        let states = vec![
            ("DOWN".into(), 200),
            ("IDLE".into(), 300),
            ("P1".into(), 100),
            ("P2".into(), 400),
        ];
        assert_eq!(residency(&states, &[1000, 2000]), (Some(50.0), Some(1800)));
        assert_eq!(residency(&states, &[1000]), (Some(50.0), None));
        assert_eq!(residency(&[("OFF".into(), 100)], &[]), (Some(0.0), None));
        assert_eq!(residency(&[("P1".into(), -1)], &[1000]), (None, None));
        assert_eq!(residency(&[], &[]), (None, None));
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    #[test]
    fn unsupported_hosts_fail_soft() {
        assert!(matches!(
            AppleBackend::try_load(),
            Err(GpuError::Unsupported(_))
        ));
    }
}

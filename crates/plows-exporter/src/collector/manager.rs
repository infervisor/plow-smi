//! Collector manager — orchestrates all enabled collectors.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::collector::{Collector, GpuSnapshot};
use crate::error::Result;

/// Manages multiple collectors and aggregates their results.
pub struct CollectorManager {
    collectors: Vec<Box<dyn Collector>>,
    /// Cached latest snapshots for prometheus scraping.
    latest_snapshots: Arc<RwLock<Vec<GpuSnapshot>>>,
}

impl CollectorManager {
    /// Create a new empty manager.
    pub fn new() -> Self {
        Self {
            collectors: Vec::new(),
            latest_snapshots: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a collector. It will be initialized immediately.
    pub async fn register(&mut self, mut collector: Box<dyn Collector>) -> Result<()> {
        let name = collector.name();
        match collector.init().await {
            Ok(device_count) => {
                info!(
                    collector = name,
                    devices = device_count,
                    "Collector initialized successfully"
                );
                self.collectors.push(collector);
            }
            Err(e) => {
                error!(collector = name, error = %e, "Failed to initialize collector");
                return Err(e);
            }
        }
        Ok(())
    }

    /// Collect metrics from all registered collectors.
    pub async fn collect_all(&self) -> Vec<GpuSnapshot> {
        let mut all_snapshots = Vec::new();

        for collector in &self.collectors {
            match collector.collect().await {
                Ok(snapshots) => {
                    all_snapshots.extend(snapshots);
                }
                Err(e) => {
                    error!(
                        collector = collector.name(),
                        error = %e,
                        "Collection failed"
                    );
                }
            }
        }

        all_snapshots
    }

    /// Run collection loop, updating cached snapshots at the given interval.
    pub async fn run_collection_loop(self: Arc<Self>, interval: std::time::Duration) {
        info!(
            interval_secs = interval.as_secs(),
            "Starting metrics collection loop"
        );

        loop {
            let snapshots = self.collect_all().await;
            {
                let mut cache = self.latest_snapshots.write().await;
                *cache = snapshots;
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// Get a read handle to the latest cached snapshots.
    pub fn latest_snapshots(&self) -> Arc<RwLock<Vec<GpuSnapshot>>> {
        Arc::clone(&self.latest_snapshots)
    }

    /// Get the number of registered collectors.
    pub fn collector_count(&self) -> usize {
        self.collectors.len()
    }
}

impl Default for CollectorManager {
    fn default() -> Self {
        Self::new()
    }
}

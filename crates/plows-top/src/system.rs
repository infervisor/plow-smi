//! System metrics collection — delegates to plows-exporter's SystemCollector.

use plows_exporter::collector::system::{
    SystemCollector as ExporterSystemCollector, SystemSnapshot,
};

/// System collector wrapping the exporter's implementation.
pub struct SystemCollector {
    inner: ExporterSystemCollector,
}

impl SystemCollector {
    pub fn new() -> Result<Self, sysinfo::Error> {
        Ok(Self {
            inner: ExporterSystemCollector::new()?,
        })
    }

    /// Collect system metrics using the exporter's collector.
    pub fn collect(&self) -> SystemSnapshot {
        self.inner.collect_system_snapshot()
    }
}

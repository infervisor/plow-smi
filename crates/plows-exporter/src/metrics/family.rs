//! Metric families built from a snapshot on every scrape.
//!
//! The `*Vec` metrics used for GPUs keep every label set until it is removed.
//! The network, WireGuard and probe collectors instead build their families
//! from the latest snapshot when Prometheus scrapes, as node_exporter does:
//! counters carry the kernel's absolute value, and an interface, peer or target
//! that goes away simply stops appearing.

use std::collections::HashMap;

use prometheus::core::Desc;
use prometheus::proto::{Counter, Gauge, LabelPair, Metric, MetricFamily, MetricType, Summary};

/// One metric family's name, help, type and label names.
pub struct Family {
    desc: Desc,
    kind: MetricType,
    labels: &'static [&'static str],
}

impl Family {
    pub fn new(name: &str, help: &str, kind: MetricType, labels: &'static [&'static str]) -> Self {
        let desc = Desc::new(
            name.to_string(),
            help.to_string(),
            labels.iter().map(|l| l.to_string()).collect(),
            HashMap::new(),
        )
        .unwrap_or_else(|e| panic!("invalid metric '{name}': {e}"));
        Self { desc, kind, labels }
    }

    pub fn counter(name: &str, help: &str, labels: &'static [&'static str]) -> Self {
        Self::new(name, help, MetricType::COUNTER, labels)
    }

    pub fn gauge(name: &str, help: &str, labels: &'static [&'static str]) -> Self {
        Self::new(name, help, MetricType::GAUGE, labels)
    }

    pub fn summary(name: &str, help: &str, labels: &'static [&'static str]) -> Self {
        Self::new(name, help, MetricType::SUMMARY, labels)
    }

    pub fn desc(&self) -> &Desc {
        &self.desc
    }

    /// Start an empty family for this scrape.
    pub fn samples(&self) -> Samples<'_> {
        let mut out = MetricFamily::default();
        out.set_name(self.desc.fq_name.clone());
        out.set_help(self.desc.help.clone());
        out.set_field_type(self.kind);
        Samples { family: self, out }
    }
}

/// The samples of one family, collected during a scrape.
pub struct Samples<'a> {
    family: &'a Family,
    out: MetricFamily,
}

impl Samples<'_> {
    fn labelled(&self, values: &[&str]) -> Metric {
        assert_eq!(
            values.len(),
            self.family.labels.len(),
            "label values for {}",
            self.family.desc.fq_name
        );
        let pairs = self
            .family
            .labels
            .iter()
            .zip(values)
            .map(|(name, value)| {
                let mut pair = LabelPair::default();
                pair.set_name(name.to_string());
                pair.set_value(value.to_string());
                pair
            })
            .collect();
        let mut metric = Metric::default();
        metric.set_label(pairs);
        metric
    }

    /// Add a counter or gauge sample.
    pub fn push(&mut self, values: &[&str], value: f64) {
        let mut metric = self.labelled(values);
        match self.family.kind {
            MetricType::COUNTER => {
                let mut c = Counter::default();
                c.set_value(value);
                metric.set_counter(c);
            }
            MetricType::GAUGE => {
                let mut g = Gauge::default();
                g.set_value(value);
                metric.set_gauge(g);
            }
            other => panic!("push on a {other:?} family"),
        }
        self.out.mut_metric().push(metric);
    }

    /// Add a summary sample with only `_sum` and `_count` (no quantiles).
    pub fn push_summary(&mut self, values: &[&str], count: u64, sum: f64) {
        assert_eq!(self.family.kind, MetricType::SUMMARY);
        let mut metric = self.labelled(values);
        let mut s = Summary::default();
        s.set_sample_count(count);
        s.set_sample_sum(sum);
        metric.set_summary(s);
        self.out.mut_metric().push(metric);
    }

    /// Append to the scrape's output; an empty family is left out.
    pub fn finish(self, into: &mut Vec<MetricFamily>) {
        if !self.out.get_metric().is_empty() {
            into.push(self.out);
        }
    }
}

/// Render collectors in the text exposition format (tests only).
#[cfg(test)]
pub fn render(collector: impl prometheus::core::Collector + 'static) -> String {
    use prometheus::Encoder;
    let registry = prometheus::Registry::new();
    registry.register(Box::new(collector)).expect("register");
    let mut buf = Vec::new();
    prometheus::TextEncoder::new()
        .encode(&registry.gather(), &mut buf)
        .expect("encode");
    String::from_utf8(buf).expect("utf8")
}

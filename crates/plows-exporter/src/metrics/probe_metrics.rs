//! `probe_*`: ICMP echo totals per target.

use std::sync::{Arc, RwLock};

use prometheus::core::{Collector, Desc};
use prometheus::proto::MetricFamily;

use super::family::Family;
use crate::collector::probe::ProbeState;

const HOST: &[&str] = &["hostname"];
const TARGET: &[&str] = &["hostname", "target"];

pub struct ProbeMetrics {
    hostname: String,
    state: Arc<RwLock<ProbeState>>,
    targets: Family,
    sent: Family,
    received: Family,
    rtt: Family,
    last_rtt: Family,
    failures: Family,
}

impl ProbeMetrics {
    pub fn new(hostname: String, state: Arc<RwLock<ProbeState>>) -> Self {
        Self {
            hostname,
            state,
            targets: Family::gauge("probe_targets", "Targets probed in the last round", HOST),
            sent: Family::counter("probe_sent_total", "ICMP echo requests sent", TARGET),
            received: Family::counter(
                "probe_received_total",
                "ICMP echo replies received in time",
                TARGET,
            ),
            rtt: Family::summary(
                "probe_rtt_seconds",
                "Round-trip time of answered probes",
                TARGET,
            ),
            last_rtt: Family::gauge(
                "probe_last_rtt_seconds",
                "Round-trip time of the last answered probe",
                TARGET,
            ),
            failures: Family::gauge(
                "probe_consecutive_failures",
                "Probes unanswered since the last reply",
                TARGET,
            ),
        }
    }

    pub fn render(&self, st: &ProbeState) -> Vec<MetricFamily> {
        let h = self.hostname.as_str();
        let mut out = Vec::new();
        let mut targets = self.targets.samples();
        targets.push(&[h], st.len() as f64);
        targets.finish(&mut out);
        let (mut sent, mut recv, mut rtt) = (
            self.sent.samples(),
            self.received.samples(),
            self.rtt.samples(),
        );
        let (mut last, mut fail) = (self.last_rtt.samples(), self.failures.samples());
        for (ip, t) in st {
            let ip = ip.to_string();
            let l = [h, ip.as_str()];
            sent.push(&l, t.sent as f64);
            recv.push(&l, t.received as f64);
            rtt.push_summary(&l, t.received, t.rtt_sum_seconds);
            if let Some(r) = t.last_rtt_seconds {
                last.push(&l, r);
            }
            fail.push(&l, t.consecutive_failures as f64);
        }
        for s in [sent, recv, rtt, last, fail] {
            s.finish(&mut out);
        }
        out
    }
}

impl Collector for ProbeMetrics {
    fn desc(&self) -> Vec<&Desc> {
        [
            &self.targets,
            &self.sent,
            &self.received,
            &self.rtt,
            &self.last_rtt,
            &self.failures,
        ]
        .into_iter()
        .map(Family::desc)
        .collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let st = self.state.read().unwrap_or_else(|e| e.into_inner()).clone();
        self.render(&st)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::probe::TargetStats;
    use crate::metrics::family::render;

    #[test]
    fn totals_summary_and_failures() {
        let mut st = ProbeState::new();
        st.insert(
            "10.100.0.1".parse().unwrap(),
            TargetStats {
                sent: 10,
                received: 9,
                rtt_sum_seconds: 0.09,
                last_rtt_seconds: Some(0.01),
                consecutive_failures: 0,
            },
        );
        st.insert(
            "10.100.0.2".parse().unwrap(),
            TargetStats {
                sent: 4,
                consecutive_failures: 4,
                ..Default::default()
            },
        );
        let t = render(ProbeMetrics::new("n1".into(), Arc::new(RwLock::new(st))));
        let a = "hostname=\"n1\",target=\"10.100.0.1\"";
        let b = "hostname=\"n1\",target=\"10.100.0.2\"";
        assert!(t.contains("probe_targets{hostname=\"n1\"} 2"), "{t}");
        assert!(t.contains(&format!("probe_sent_total{{{a}}} 10")));
        assert!(t.contains(&format!("probe_received_total{{{a}}} 9")));
        assert!(t.contains("# TYPE probe_rtt_seconds summary"));
        assert!(t.contains(&format!("probe_rtt_seconds_count{{{a}}} 9")));
        assert!(t.contains(&format!("probe_last_rtt_seconds{{{a}}} 0.01")));
        assert!(
            !t.contains(&format!("probe_last_rtt_seconds{{{b}}}")),
            "never answered: no last rtt"
        );
        assert!(t.contains(&format!("probe_consecutive_failures{{{b}}} 4")));
    }
}

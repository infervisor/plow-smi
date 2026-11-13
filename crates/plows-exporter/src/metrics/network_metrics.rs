//! `network_*`: per-interface counters, read from the host at scrape time.
use std::sync::atomic::{AtomicBool, Ordering};

use prometheus::core::{Collector, Desc};
use prometheus::proto::MetricFamily;
use tracing::warn;

use super::family::Family;
use crate::collector::network::{read_interfaces, InterfaceFilter, InterfaceStats};

const L: &[&str] = &["hostname", "interface"];
const INFO: &[&str] = &["hostname", "interface", "mac", "kind"];

/// Reads one counter off an interface.
type Field = fn(&InterfaceStats) -> Option<u64>;

pub struct NetworkMetrics {
    hostname: String,
    filter: InterfaceFilter,
    warned: AtomicBool,
    counters: Vec<(Family, Field)>,
    up: Family,
    carrier: Family,
    mtu: Family,
    speed: Family,
    info: Family,
}

impl NetworkMetrics {
    pub fn new(hostname: String, filter: InterfaceFilter) -> Self {
        let c = |name: &str, help: &str| Family::counter(name, help, L);
        Self {
            hostname,
            filter,
            warned: AtomicBool::new(false),
            counters: vec![
                (c("network_receive_bytes_total", "Bytes received"), |i| {
                    i.rx_bytes
                }),
                (
                    c("network_transmit_bytes_total", "Bytes transmitted"),
                    |i| i.tx_bytes,
                ),
                (
                    c("network_receive_packets_total", "Packets received"),
                    |i| i.rx_packets,
                ),
                (
                    c("network_transmit_packets_total", "Packets transmitted"),
                    |i| i.tx_packets,
                ),
                (c("network_receive_errors_total", "Receive errors"), |i| {
                    i.rx_errors
                }),
                (c("network_transmit_errors_total", "Transmit errors"), |i| {
                    i.tx_errors
                }),
                (
                    c("network_receive_drops_total", "Received packets dropped"),
                    |i| i.rx_drops,
                ),
                (
                    c(
                        "network_transmit_drops_total",
                        "Transmitted packets dropped",
                    ),
                    |i| i.tx_drops,
                ),
            ],
            up: Family::gauge("network_up", "1 if the interface is operational", L),
            carrier: Family::gauge("network_carrier", "1 if the link has a carrier", L),
            mtu: Family::gauge("network_mtu_bytes", "Interface MTU in bytes", L),
            speed: Family::gauge("network_speed_bytes", "Link speed in bytes per second", L),
            info: Family::gauge("network_info", "Interface identity; always 1", INFO),
        }
    }

    fn families(&self) -> impl Iterator<Item = &Family> {
        self.counters.iter().map(|(f, _)| f).chain([
            &self.up,
            &self.carrier,
            &self.mtu,
            &self.speed,
            &self.info,
        ])
    }

    /// The text families for these interfaces.
    pub fn render(&self, ifaces: &[InterfaceStats]) -> Vec<MetricFamily> {
        let mut out = Vec::new();
        let h = self.hostname.as_str();
        for (family, field) in &self.counters {
            let mut s = family.samples();
            for i in ifaces {
                if let Some(v) = field(i) {
                    s.push(&[h, &i.name], v as f64);
                }
            }
            s.finish(&mut out);
        }
        let gauge =
            |family: &Family, field: &dyn Fn(&InterfaceStats) -> Option<f64>, out: &mut Vec<_>| {
                let mut s = family.samples();
                for i in ifaces {
                    if let Some(v) = field(i) {
                        s.push(&[h, &i.name], v);
                    }
                }
                s.finish(out);
            };
        gauge(&self.up, &|i| Some(f64::from(u8::from(i.up))), &mut out);
        gauge(
            &self.carrier,
            &|i| i.carrier.map(|c| f64::from(u8::from(c))),
            &mut out,
        );
        gauge(&self.mtu, &|i| i.mtu.map(|v| v as f64), &mut out);
        gauge(&self.speed, &|i| i.speed_bytes.map(|v| v as f64), &mut out);
        let mut s = self.info.samples();
        for i in ifaces {
            s.push(&[h, &i.name, &i.mac, &i.kind], 1.0);
        }
        s.finish(&mut out);
        out
    }
}

impl Collector for NetworkMetrics {
    fn desc(&self) -> Vec<&Desc> {
        self.families().map(Family::desc).collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        match read_interfaces(&self.filter) {
            Ok(ifaces) => self.render(&ifaces),
            Err(e) => {
                if !self.warned.swap(true, Ordering::Relaxed) {
                    warn!(error = %e, "network interface counters unavailable");
                }
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::family::render;

    struct Fixed(NetworkMetrics, Vec<InterfaceStats>);
    impl Collector for Fixed {
        fn desc(&self) -> Vec<&Desc> {
            self.0.desc()
        }
        fn collect(&self) -> Vec<MetricFamily> {
            self.0.render(&self.1)
        }
    }

    fn metrics() -> NetworkMetrics {
        NetworkMetrics::new("h1".into(), InterfaceFilter::default())
    }

    #[test]
    fn counters_are_counters_and_missing_fields_are_absent() {
        let eth = InterfaceStats {
            name: "eth0".into(),
            kind: "ether".into(),
            mac: "aa:bb".into(),
            rx_bytes: Some(1000),
            rx_drops: Some(2),
            up: true,
            mtu: Some(1500),
            ..Default::default()
        };
        let text = render(Fixed(metrics(), vec![eth]));
        assert!(
            text.contains("# TYPE network_receive_bytes_total counter"),
            "{text}"
        );
        assert!(
            text.contains("network_receive_bytes_total{hostname=\"h1\",interface=\"eth0\"} 1000")
        );
        assert!(text.contains("network_receive_drops_total{hostname=\"h1\",interface=\"eth0\"} 2"));
        assert!(text.contains("network_up{hostname=\"h1\",interface=\"eth0\"} 1"));
        assert!(text.contains(
            "network_info{hostname=\"h1\",interface=\"eth0\",mac=\"aa:bb\",kind=\"ether\"} 1"
        ));
        assert!(
            !text.contains("network_transmit_bytes_total"),
            "absent counter is left out"
        );
        assert!(!text.contains("network_speed_bytes"));
    }

    #[test]
    fn an_interface_that_goes_away_leaves_the_output() {
        let text = render(Fixed(metrics(), Vec::new()));
        assert!(text.is_empty(), "{text}");
    }
}

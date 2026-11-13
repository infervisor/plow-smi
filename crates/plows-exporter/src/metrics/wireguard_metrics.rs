//! `wireguard_*`: interfaces and peers from the last `wg show all dump`.

use std::sync::{Arc, RwLock};

use prometheus::core::{Collector, Desc};
use prometheus::proto::MetricFamily;

use super::family::Family;
use crate::collector::wireguard::WgState;

const HOST: &[&str] = &["hostname"];
const IFACE: &[&str] = &["hostname", "interface"];
const IFACE_INFO: &[&str] = &["hostname", "interface", "public_key", "listen_port"];
const PEER: &[&str] = &["hostname", "interface", "peer"];
const ALLOWED: &[&str] = &["hostname", "interface", "peer", "allowed_ip"];

pub struct WireGuardMetrics {
    hostname: String,
    state: Arc<RwLock<WgState>>,
    success: Family,
    iface_info: Family,
    peers: Family,
    handshake: Family,
    rx: Family,
    tx: Family,
    keepalive: Family,
    has_endpoint: Family,
    allowed: Family,
}

impl WireGuardMetrics {
    pub fn new(hostname: String, state: Arc<RwLock<WgState>>) -> Self {
        Self {
            hostname,
            state,
            success: Family::gauge(
                "wireguard_read_success",
                "1 if the last `wg show all dump` succeeded",
                HOST,
            ),
            iface_info: Family::gauge(
                "wireguard_interface_info",
                "WireGuard interface; always 1",
                IFACE_INFO,
            ),
            peers: Family::gauge(
                "wireguard_peers",
                "Peers configured on the interface",
                IFACE,
            ),
            handshake: Family::gauge(
                "wireguard_peer_latest_handshake_seconds",
                "Unix time of the last completed handshake; 0 if never",
                PEER,
            ),
            rx: Family::counter(
                "wireguard_peer_receive_bytes_total",
                "Bytes received from the peer",
                PEER,
            ),
            tx: Family::counter(
                "wireguard_peer_transmit_bytes_total",
                "Bytes sent to the peer",
                PEER,
            ),
            keepalive: Family::gauge(
                "wireguard_peer_persistent_keepalive_seconds",
                "Persistent keepalive interval, when set",
                PEER,
            ),
            has_endpoint: Family::gauge(
                "wireguard_peer_has_endpoint",
                "1 if the peer's endpoint is known",
                PEER,
            ),
            allowed: Family::gauge(
                "wireguard_peer_allowed_ip_info",
                "One allowed IP of the peer; always 1",
                ALLOWED,
            ),
        }
    }

    pub fn render(&self, st: &WgState) -> Vec<MetricFamily> {
        let h = self.hostname.as_str();
        let mut out = Vec::new();
        let mut success = self.success.samples();
        success.push(&[h], f64::from(u8::from(st.ok)));
        success.finish(&mut out);

        let (mut info, mut peers) = (self.iface_info.samples(), self.peers.samples());
        let (mut hs, mut rx, mut tx) = (
            self.handshake.samples(),
            self.rx.samples(),
            self.tx.samples(),
        );
        let (mut ka, mut ep, mut al) = (
            self.keepalive.samples(),
            self.has_endpoint.samples(),
            self.allowed.samples(),
        );
        for iface in &st.interfaces {
            let i = iface.name.as_str();
            info.push(
                &[h, i, &iface.public_key, &iface.listen_port.to_string()],
                1.0,
            );
            peers.push(&[h, i], iface.peers.len() as f64);
            for p in &iface.peers {
                let l = [h, i, p.public_key.as_str()];
                hs.push(&l, p.latest_handshake as f64);
                rx.push(&l, p.rx_bytes as f64);
                tx.push(&l, p.tx_bytes as f64);
                ep.push(&l, f64::from(u8::from(p.has_endpoint)));
                if let Some(k) = p.persistent_keepalive {
                    ka.push(&l, k as f64);
                }
                for ip in &p.allowed_ips {
                    al.push(&[h, i, &p.public_key, ip], 1.0);
                }
            }
        }
        for s in [info, peers, hs, rx, tx, ka, ep, al] {
            s.finish(&mut out);
        }
        out
    }
}

impl Collector for WireGuardMetrics {
    fn desc(&self) -> Vec<&Desc> {
        [
            &self.success,
            &self.iface_info,
            &self.peers,
            &self.handshake,
            &self.rx,
            &self.tx,
            &self.keepalive,
            &self.has_endpoint,
            &self.allowed,
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
    use crate::collector::wireguard::parse_dump;
    use crate::metrics::family::render;

    fn text(st: WgState) -> String {
        render(WireGuardMetrics::new(
            "hub".into(),
            Arc::new(RwLock::new(st)),
        ))
    }

    #[test]
    fn peers_handshakes_and_transfer() {
        let interfaces = parse_dump(include_str!("../../tests/fixtures/wg-dump.txt")).unwrap();
        let t = text(WgState {
            ok: true,
            interfaces,
        });
        let peer = "hostname=\"hub\",interface=\"inferos0\",peer=\"Tm9kZUFQdWJsaWNLZXlBQUFBQUFBQUFBQUFBQUFBQUE=\"";
        assert!(
            t.contains("wireguard_read_success{hostname=\"hub\"} 1"),
            "{t}"
        );
        assert!(t.contains("wireguard_peers{hostname=\"hub\",interface=\"inferos0\"} 3"));
        assert!(t.contains(&format!(
            "wireguard_peer_latest_handshake_seconds{{{peer}}} 1762900000"
        )));
        assert!(t.contains("# TYPE wireguard_peer_receive_bytes_total counter"));
        assert!(t.contains(&format!(
            "wireguard_peer_receive_bytes_total{{{peer}}} 123456"
        )));
        assert!(t.contains(&format!(
            "wireguard_peer_allowed_ip_info{{{peer},allowed_ip=\"100.64.0.7/32\"}} 1"
        )));
        assert!(t.contains("listen_port=\"51820\""));
        // A never-handshaken peer is reported as 0, not left out.
        assert!(t.contains("peer=\"Tm9kZUNQdWJsaWNLZXlBQUFBQUFBQUFBQUFBQUFBQUE=\"} 0"));
        assert!(!t.contains("203.0.113.5"), "no endpoint addresses");
    }

    #[test]
    fn a_failed_read_says_so_and_has_no_peers() {
        let t = text(WgState::default());
        assert_eq!(
            t.lines()
                .filter(|l| !l.starts_with('#'))
                .collect::<Vec<_>>(),
            ["wireguard_read_success{hostname=\"hub\"} 0"]
        );
    }
}

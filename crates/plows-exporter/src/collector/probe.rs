//! ICMP echo probes: loss and round-trip time to chosen IPv4 targets.
//!
//! One echo per target per round on one socket. An unprivileged ping socket
//! (`SOCK_DGRAM`, allowed by `net.ipv4.ping_group_range`) is tried first, then
//! a raw socket (needs `CAP_NET_RAW`). Replies are matched by source address,
//! sequence number and a per-process token in the payload.
//!
//! Counters are kept as totals; loss is computed in PromQL, so nothing is
//! lost to an averaging window chosen here.

use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tracing::{debug, info, warn};

use super::wireguard::{WgInterface, WgState};

/// Probing more targets than this logs once and probes the first ones.
pub const MAX_TARGETS: usize = 256;
/// The longest a reply is waited for.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// What to probe and how often.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub targets: Vec<Ipv4Addr>,
    /// Also probe every WireGuard peer (see [`wireguard_peer_target`]).
    pub wireguard_peers: bool,
    pub interval: Duration,
}

/// Totals for one target since it was first probed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetStats {
    pub sent: u64,
    pub received: u64,
    pub rtt_sum_seconds: f64,
    pub last_rtt_seconds: Option<f64>,
    pub consecutive_failures: u64,
}

pub type ProbeState = BTreeMap<Ipv4Addr, TargetStats>;

/// The address that answers for a peer: its first single-host IPv4 allowed IP.
/// A peer routing a whole subnet (no `/32`) has no single address to ping.
pub fn wireguard_peer_target(allowed_ips: &[String]) -> Option<Ipv4Addr> {
    allowed_ips
        .iter()
        .find_map(|cidr| cidr.strip_suffix("/32").and_then(|ip| ip.parse().ok()))
}

/// This round's targets: the configured ones plus, if asked, one per peer;
/// sorted, without duplicates, at most [`MAX_TARGETS`].
pub fn round_targets(fixed: &[Ipv4Addr], wg: Option<&[WgInterface]>) -> Vec<Ipv4Addr> {
    let mut all: Vec<Ipv4Addr> = fixed.to_vec();
    for iface in wg.unwrap_or_default() {
        all.extend(
            iface
                .peers
                .iter()
                .filter_map(|p| wireguard_peer_target(&p.allowed_ips)),
        );
    }
    all.sort();
    all.dedup();
    all.truncate(MAX_TARGETS);
    all
}

/// An ICMP echo socket.
pub struct Pinger {
    socket: UdpSocket,
    /// A raw socket sees every echo reply on the host, so the identifier is
    /// checked. A ping socket's identifier is rewritten by the kernel, which
    /// delivers it only that socket's replies.
    raw: bool,
    ident: u16,
    token: [u8; 8],
}

impl Pinger {
    pub fn open() -> std::io::Result<Self> {
        let (socket, raw) = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)) {
            Ok(s) => (s, false),
            Err(dgram) => match Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)) {
                Ok(s) => (s, true),
                Err(raw) => {
                    return Err(std::io::Error::new(
                        raw.kind(),
                        format!("ping socket: {dgram}; raw socket: {raw}"),
                    ))
                }
            },
        };
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Ok(Self {
            // A datagram socket for sending and receiving, without unsafe code:
            // the source address comes back as an IPv4 socket address.
            socket: socket.into(),
            raw,
            ident: std::process::id() as u16,
            token: (nanos ^ ((std::process::id() as u64) << 32)).to_be_bytes(),
        })
    }

    pub fn is_raw(&self) -> bool {
        self.raw
    }

    fn send(&self, target: Ipv4Addr, seq: u16) -> std::io::Result<()> {
        let packet = echo_request(self.ident, seq, &self.token);
        self.socket
            .send_to(&packet, SocketAddrV4::new(target, 0))
            .map(|_| ())
    }

    /// The next matching reply, or `None` once `deadline` passes.
    fn recv(&self, deadline: Instant) -> Option<(Ipv4Addr, u16)> {
        let mut buf = [0u8; 1500];
        loop {
            let left = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())?;
            if self.socket.set_read_timeout(Some(left)).is_err() {
                return None;
            }
            match self.socket.recv_from(&mut buf) {
                Ok((n, SocketAddr::V4(from))) => {
                    let ident = self.raw.then_some(self.ident);
                    if let Some(seq) = parse_reply(&buf[..n], ident, &self.token) {
                        return Some((*from.ip(), seq));
                    }
                }
                Ok(_) => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return None
                }
                // An ICMP error (host unreachable…) queued on the socket: that
                // probe is simply lost. Keep reading for the others.
                Err(e) => debug!(error = %e, "probe receive error"),
            }
        }
    }
}

/// An ICMP echo request: type 8, code 0, identifier, sequence, token payload.
fn echo_request(ident: u16, seq: u16, token: &[u8; 8]) -> [u8; 16] {
    let mut p = [0u8; 16];
    p[0] = 8;
    p[4..6].copy_from_slice(&ident.to_be_bytes());
    p[6..8].copy_from_slice(&seq.to_be_bytes());
    p[8..16].copy_from_slice(token);
    let sum = checksum(&p);
    p[2..4].copy_from_slice(&sum.to_be_bytes());
    p
}

/// The sequence number of a reply to one of this process's requests.
///
/// A raw socket (and a macOS ping socket) returns the IPv4 header first; a
/// Linux ping socket returns only the ICMP message. An ICMP message never
/// starts with 0x4_, an IPv4 header always does.
fn parse_reply(buf: &[u8], ident: Option<u16>, token: &[u8; 8]) -> Option<u16> {
    let icmp = match buf.first() {
        Some(b) if b >> 4 == 4 => buf.get(usize::from(b & 0x0f) * 4..)?,
        _ => buf,
    };
    if icmp.len() < 16 || icmp[0] != 0 || icmp[1] != 0 || &icmp[8..16] != token {
        return None;
    }
    if ident.is_some_and(|id| id.to_be_bytes() != icmp[4..6]) {
        return None;
    }
    Some(u16::from_be_bytes([icmp[6], icmp[7]]))
}

/// RFC 1071 Internet checksum.
fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data
        .chunks(2)
        .map(|c| u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])))
        .sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Probe forever, publishing totals to `state` after every round. Returns
/// only if no ICMP socket can be opened.
pub fn run(cfg: ProbeConfig, wg: Option<Arc<RwLock<WgState>>>, state: Arc<RwLock<ProbeState>>) {
    let pinger = match Pinger::open() {
        Ok(p) => p,
        Err(e) => {
            warn!(
                error = %e,
                "probing disabled: allow ping sockets (net.ipv4.ping_group_range) or grant CAP_NET_RAW"
            );
            return;
        }
    };
    info!(raw = pinger.is_raw(), "ICMP probing started");
    let wait = cfg.interval.min(MAX_WAIT);
    let mut stats = ProbeState::new();
    let mut seqs: HashMap<Ipv4Addr, u16> = HashMap::new();
    let mut capped = false;

    loop {
        let start = Instant::now();
        let peers = wg.as_ref().filter(|_| cfg.wireguard_peers).map(|w| {
            w.read()
                .unwrap_or_else(|e| e.into_inner())
                .interfaces
                .clone()
        });
        let targets = round_targets(&cfg.targets, peers.as_deref());
        if targets.len() == MAX_TARGETS && !capped {
            warn!(
                max = MAX_TARGETS,
                "too many probe targets; probing the first ones"
            );
            capped = true;
        }
        // A target no longer wanted (a removed peer) drops out of the output.
        stats.retain(|ip, _| targets.contains(ip));
        seqs.retain(|ip, _| targets.contains(ip));

        let mut pending: HashMap<(Ipv4Addr, u16), Instant> = HashMap::new();
        for &target in &targets {
            let seq = seqs.entry(target).or_default();
            *seq = seq.wrapping_add(1);
            stats.entry(target).or_default().sent += 1;
            let sent_at = Instant::now();
            if let Err(e) = pinger.send(target, *seq) {
                debug!(%target, error = %e, "probe send failed");
            }
            pending.insert((target, *seq), sent_at);
        }

        let deadline = start + wait;
        while !pending.is_empty() {
            let Some(reply) = pinger.recv(deadline) else {
                break;
            };
            if let Some(sent_at) = pending.remove(&reply) {
                let rtt = sent_at.elapsed().as_secs_f64();
                let t = stats.entry(reply.0).or_default();
                t.received += 1;
                t.rtt_sum_seconds += rtt;
                t.last_rtt_seconds = Some(rtt);
                t.consecutive_failures = 0;
            }
        }
        for (target, _) in pending.keys() {
            stats.entry(*target).or_default().consecutive_failures += 1;
        }

        *state.write().unwrap_or_else(|e| e.into_inner()) = stats.clone();
        if let Some(rest) = (start + cfg.interval).checked_duration_since(Instant::now()) {
            std::thread::sleep(rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::wireguard::parse_dump;

    const TOKEN: [u8; 8] = *b"plowsmi!";

    fn reply(ident: u16, seq: u16, token: &[u8; 8]) -> Vec<u8> {
        let mut r = echo_request(ident, seq, token).to_vec();
        r[0] = 0;
        r
    }

    #[test]
    fn a_request_checksums_to_zero() {
        assert_eq!(checksum(&echo_request(0x1234, 7, &TOKEN)), 0);
    }

    #[test]
    fn a_ping_socket_reply_matches_on_token_alone() {
        assert_eq!(parse_reply(&reply(999, 42, &TOKEN), None, &TOKEN), Some(42));
        assert_eq!(
            parse_reply(&reply(999, 42, b"another!"), None, &TOKEN),
            None
        );
        // An echo request (type 8) is not a reply.
        assert_eq!(parse_reply(&echo_request(1, 1, &TOKEN), None, &TOKEN), None);
        assert_eq!(parse_reply(&[0, 0, 0], None, &TOKEN), None);
    }

    #[test]
    fn a_raw_reply_skips_the_ip_header_and_checks_the_identifier() {
        let mut packet = vec![
            0x45, 0, 0, 36, 0, 0, 0, 0, 64, 1, 0, 0, 10, 0, 0, 2, 10, 0, 0, 1,
        ];
        packet.extend(reply(77, 5, &TOKEN));
        assert_eq!(parse_reply(&packet, Some(77), &TOKEN), Some(5));
        assert_eq!(parse_reply(&packet, Some(78), &TOKEN), None);
        // A truncated header cannot be read past.
        assert_eq!(parse_reply(&[0x4f, 0, 0], Some(77), &TOKEN), None);
    }

    #[test]
    fn peer_targets_are_single_host_ipv4_addresses() {
        let ips = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            wireguard_peer_target(&ips(&["10.100.0.0/24", "10.100.0.9/32"])),
            Some(Ipv4Addr::new(10, 100, 0, 9))
        );
        assert_eq!(
            wireguard_peer_target(&ips(&["10.100.0.0/24", "fd00::1/128"])),
            None
        );
        assert_eq!(wireguard_peer_target(&[]), None);
    }

    #[test]
    fn round_targets_merge_sort_and_dedupe() {
        let wg = parse_dump(include_str!("../../tests/fixtures/wg-dump.txt")).unwrap();
        let fixed = [Ipv4Addr::new(10, 100, 0, 3), Ipv4Addr::new(1, 1, 1, 1)];
        let got = round_targets(&fixed, Some(&wg));
        let want: Vec<Ipv4Addr> = ["1.1.1.1", "10.100.0.2", "10.100.0.3", "100.64.0.1"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(got, want);
        assert_eq!(round_targets(&fixed, None).len(), 2);
    }

    #[test]
    fn round_targets_are_capped() {
        let many: Vec<Ipv4Addr> = (0..400u32)
            .map(|i| Ipv4Addr::from(0x0a00_0000 + i))
            .collect();
        assert_eq!(round_targets(&many, None).len(), MAX_TARGETS);
    }

    /// Needs ping sockets or CAP_NET_RAW: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn loopback_answers() {
        let state = Arc::new(RwLock::new(ProbeState::new()));
        let s = Arc::clone(&state);
        let cfg = ProbeConfig {
            targets: vec![Ipv4Addr::LOCALHOST],
            wireguard_peers: false,
            interval: Duration::from_millis(200),
        };
        std::thread::spawn(move || run(cfg, None, s));
        std::thread::sleep(Duration::from_millis(1100));
        let st = state.read().unwrap()[&Ipv4Addr::LOCALHOST].clone();
        assert!(st.sent >= 3 && st.received >= 3, "{st:?}");
        assert_eq!(st.consecutive_failures, 0);
    }
}

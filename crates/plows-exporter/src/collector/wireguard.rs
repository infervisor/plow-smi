//! WireGuard interfaces and peers, from `wg show all dump`.
//!
//! The dump is the stable, tab-separated format `wg` documents for scripts.
//! It includes each interface's private key and every preshared key: the
//! parser discards those columns, so no struct here can hold a secret, and a
//! parse error names a line number, never the line.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The most `wg` output read; a larger dump is refused.
const MAX_OUTPUT: u64 = 1 << 20;
/// How long `wg` may take before it is killed.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Checked after `PATH`: a systemd unit's `PATH` is often minimal.
const FALLBACK_DIRS: &[&str] = &[
    "/run/current-system/sw/bin",
    "/usr/bin",
    "/usr/local/bin",
    "/opt/homebrew/bin",
];

#[derive(Debug, Clone, PartialEq)]
pub struct WgInterface {
    pub name: String,
    pub public_key: String,
    pub listen_port: u16,
    pub peers: Vec<WgPeer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgPeer {
    pub public_key: String,
    /// Whether the peer has an endpoint. The address itself is not kept: it
    /// is a public IP that changes with NAT and does not belong in a label.
    pub has_endpoint: bool,
    pub allowed_ips: Vec<String>,
    /// Unix seconds of the last completed handshake; 0 means never.
    pub latest_handshake: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub persistent_keepalive: Option<u64>,
}

/// The last read: `ok` is false when `wg` failed (the interfaces are then empty).
#[derive(Debug, Clone, Default)]
pub struct WgState {
    pub ok: bool,
    pub interfaces: Vec<WgInterface>,
}

/// Parse `wg show all dump`.
pub fn parse_dump(text: &str) -> Result<Vec<WgInterface>, String> {
    let mut out: Vec<WgInterface> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let bad = |what: &str| format!("wg dump line {}: {what}", i + 1);
        match f.len() {
            // interface, private key (dropped), public key, listen port, fwmark
            5 => out.push(WgInterface {
                name: f[0].to_string(),
                public_key: f[2].to_string(),
                listen_port: f[3].parse().map_err(|_| bad("listen port"))?,
                peers: Vec::new(),
            }),
            // interface, public key, preshared key (dropped), endpoint,
            // allowed ips, latest handshake, rx, tx, persistent keepalive
            9 => {
                let iface = out
                    .iter_mut()
                    .rev()
                    .find(|w| w.name == f[0])
                    .ok_or_else(|| bad("peer before its interface"))?;
                let number = |s: &str, what: &str| s.parse::<u64>().map_err(|_| bad(what));
                iface.peers.push(WgPeer {
                    public_key: f[1].to_string(),
                    has_endpoint: f[3] != "(none)",
                    allowed_ips: match f[4] {
                        "(none)" | "" => Vec::new(),
                        list => list.split(',').map(|s| s.trim().to_string()).collect(),
                    },
                    latest_handshake: number(f[5], "latest handshake")?,
                    rx_bytes: number(f[6], "transfer rx")?,
                    tx_bytes: number(f[7], "transfer tx")?,
                    persistent_keepalive: match f[8] {
                        "off" => None,
                        s => Some(number(s, "persistent keepalive")?),
                    },
                });
            }
            n => return Err(bad(&format!("{n} fields"))),
        }
    }
    Ok(out)
}

/// The `wg` binary.
#[derive(Debug, Clone)]
pub struct WgCommand {
    binary: PathBuf,
}

impl WgCommand {
    /// Find `wg` on `PATH` or in the usual install directories.
    pub fn locate() -> Option<Self> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .chain(FALLBACK_DIRS.iter().map(PathBuf::from))
            .map(|d| d.join("wg"))
            .find(|p| is_executable(p))
            .map(|binary| Self { binary })
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Run `wg show all dump` and parse it.
    pub fn read(&self) -> Result<Vec<WgInterface>, String> {
        let mut child = Command::new(&self.binary)
            .args(["show", "all", "dump"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.binary.display()))?;
        let reader = |pipe: Option<Box<dyn Read + Send>>, cap: u64| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(p) = pipe {
                    let _ = p.take(cap).read_to_end(&mut buf);
                }
                buf
            })
        };
        let stdout = reader(
            child.stdout.take().map(|p| Box::new(p) as _),
            MAX_OUTPUT + 1,
        );
        let stderr = reader(child.stderr.take().map(|p| Box::new(p) as _), 4096);

        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let out = stdout.join().unwrap_or_default();
        let err = stderr.join().unwrap_or_default();
        match status {
            None => Err(format!("wg did not finish within {TIMEOUT:?}")),
            Some(_) if out.len() as u64 > MAX_OUTPUT => Err("wg output over 1 MiB".into()),
            Some(s) if !s.success() => {
                let first = String::from_utf8_lossy(&err);
                Err(format!(
                    "wg failed ({s}): {}",
                    first.lines().next().unwrap_or("").trim()
                ))
            }
            Some(_) => parse_dump(&String::from_utf8_lossy(&out)),
        }
    }
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const DUMP: &str = include_str!("../../tests/fixtures/wg-dump.txt");
    const PRIVATE: &str = "cHJpdmF0ZS1rZXktbmV2ZXItZXhwb3J0ZWQtYWFhYWE=";
    const PRESHARED: &str = "cHJlc2hhcmVkLWtleS1uZXZlci1leHBvcnRlZC1iYmI=";

    #[test]
    fn parses_interfaces_and_peers() {
        let wg = parse_dump(DUMP).unwrap();
        assert_eq!(wg.len(), 2);
        let hub = &wg[0];
        assert_eq!((hub.name.as_str(), hub.listen_port), ("inferos0", 443));
        assert_eq!(hub.peers.len(), 3);

        let a = &hub.peers[0];
        assert!(a.has_endpoint);
        assert_eq!(a.allowed_ips, ["10.100.0.2/32", "100.64.0.7/32"]);
        assert_eq!(
            (a.latest_handshake, a.rx_bytes, a.tx_bytes),
            (1762900000, 123456, 654321)
        );
        assert_eq!(a.persistent_keepalive, Some(25));

        let never = &hub.peers[2];
        assert!(!never.has_endpoint);
        assert_eq!(never.latest_handshake, 0);
        assert_eq!(never.persistent_keepalive, None);
        assert!(never.allowed_ips.is_empty());

        assert_eq!(wg[1].peers.len(), 1);
    }

    #[test]
    fn secrets_never_reach_the_parsed_value() {
        assert!(
            DUMP.contains(PRIVATE) && DUMP.contains(PRESHARED),
            "fixture carries both"
        );
        let debug = format!("{:?}", parse_dump(DUMP).unwrap());
        assert!(!debug.contains(PRIVATE) && !debug.contains(PRESHARED));
        assert!(
            !debug.contains("203.0.113.5"),
            "endpoint address is not kept"
        );
    }

    #[test]
    fn a_bad_line_is_an_error_that_does_not_echo_it() {
        let err = parse_dump(&format!("wg0\t{PRIVATE}\tpub\tnot-a-port\toff\n")).unwrap_err();
        assert!(err.contains("line 1") && !err.contains(PRIVATE));
        assert!(parse_dump("wg0\ta\tb\n").is_err());
        assert!(parse_dump("wg0\tp\t(none)\t(none)\t(none)\t0\t0\t0\toff\n").is_err());
    }

    #[test]
    fn empty_output_is_no_interfaces() {
        assert_eq!(parse_dump("").unwrap(), Vec::new());
    }
}

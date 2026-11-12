//! Network interface counters from `/sys/class/net` (Linux).
//!
//! Read at scrape time: a sysfs read is cheap, and the counters then match
//! the moment Prometheus records them. Other platforms have no `/sys`; the
//! reader returns nothing there and `system_network_*` still carries byte totals.

use std::fs;
use std::path::Path;

/// Where Linux publishes interface state.
pub const SYSFS_NET: &str = "/sys/class/net";

/// Interfaces skipped unless `--network-include` names them: loopback and the
/// per-container or per-bridge links that come and go with workloads.
const DEFAULT_EXCLUDE: &[&str] = &[
    "lo",
    "veth*",
    "docker*",
    "br-*",
    "virbr*",
    "cni*",
    "flannel*",
    "cali*",
    "vxlan.calico",
    "tunl0",
    "kube-ipvs*",
];

/// One interface's counters and state. Every field the kernel did not expose
/// is `None` and left out of the output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InterfaceStats {
    pub name: String,
    /// `DEVTYPE` from uevent (`wireguard`, `bridge`, `wlan`…), else the link type.
    pub kind: String,
    pub mac: String,
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub rx_packets: Option<u64>,
    pub tx_packets: Option<u64>,
    pub rx_errors: Option<u64>,
    pub tx_errors: Option<u64>,
    pub rx_drops: Option<u64>,
    pub tx_drops: Option<u64>,
    pub up: bool,
    pub carrier: Option<bool>,
    pub mtu: Option<u64>,
    /// Link speed in bytes per second, when the driver reports one.
    pub speed_bytes: Option<u64>,
}

/// Which interfaces to report.
#[derive(Debug, Clone, Default)]
pub struct InterfaceFilter {
    include: Vec<String>,
}

impl InterfaceFilter {
    pub fn new(include: Vec<String>) -> Self {
        Self { include }
    }

    pub fn keep(&self, name: &str) -> bool {
        self.include.iter().any(|p| glob(p, name)) || !DEFAULT_EXCLUDE.iter().any(|p| glob(p, name))
    }
}

/// Shell-style match with `*` (any run) and `?` (one character).
pub fn glob(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Read every kept interface under `root` (normally [`SYSFS_NET`]), sorted by name.
pub fn read_interfaces(
    root: &Path,
    filter: &InterfaceFilter,
) -> std::io::Result<Vec<InterfaceStats>> {
    let mut names: Vec<String> = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| filter.keep(n))
        .collect();
    names.sort();
    Ok(names
        .into_iter()
        .map(|n| read_interface(&root.join(&n), n))
        .collect())
}

fn read_interface(dir: &Path, name: String) -> InterfaceStats {
    let text = |file: &str| {
        fs::read_to_string(dir.join(file))
            .ok()
            .map(|s| s.trim().to_string())
    };
    let number = |file: &str| text(file).and_then(|s| s.parse::<u64>().ok());
    let stat = |file: &str| number(&format!("statistics/{file}"));

    // Reading `carrier` fails (EINVAL) while the link is administratively down.
    let carrier = number("carrier").map(|c| c == 1);
    // A WireGuard or loopback link reports "unknown": up is then its carrier.
    let up = match text("operstate").as_deref() {
        Some("up") => true,
        Some("unknown") => carrier.unwrap_or(false),
        _ => false,
    };
    let kind = text("uevent")
        .and_then(|u| {
            u.lines()
                .find_map(|l| l.strip_prefix("DEVTYPE=").map(str::to_string))
        })
        .unwrap_or_else(|| link_type(number("type")));
    // `speed` is in Mb/s; virtual links report -1 or fail to read.
    let speed_bytes = text("speed")
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&mbps| mbps > 0)
        .map(|mbps| mbps as u64 * 125_000);

    InterfaceStats {
        kind,
        mac: text("address").unwrap_or_default(),
        rx_bytes: stat("rx_bytes"),
        tx_bytes: stat("tx_bytes"),
        rx_packets: stat("rx_packets"),
        tx_packets: stat("tx_packets"),
        rx_errors: stat("rx_errors"),
        tx_errors: stat("tx_errors"),
        rx_drops: stat("rx_dropped"),
        tx_drops: stat("tx_dropped"),
        up,
        carrier,
        mtu: number("mtu"),
        speed_bytes,
        name,
    }
}

/// Name the common `ARPHRD_*` link types (`include/uapi/linux/if_arp.h`).
fn link_type(arphrd: Option<u64>) -> String {
    match arphrd {
        Some(1) => "ether".into(),
        Some(32) => "infiniband".into(),
        Some(772) => "loopback".into(),
        Some(65534) => "none".into(),
        Some(n) => n.to_string(),
        None => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, file: &str, value: &str) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{value}\n")).unwrap();
    }

    fn fake_sysfs() -> tempdir::TempDir {
        let root = tempdir::TempDir::new();
        let eth = root.path().join("eth0");
        for (f, v) in [
            ("statistics/rx_bytes", "1000"),
            ("statistics/tx_bytes", "2000"),
            ("statistics/rx_packets", "10"),
            ("statistics/tx_packets", "20"),
            ("statistics/rx_errors", "1"),
            ("statistics/tx_errors", "0"),
            ("statistics/rx_dropped", "3"),
            ("statistics/tx_dropped", "0"),
            ("operstate", "up"),
            ("carrier", "1"),
            ("mtu", "1500"),
            ("speed", "10000"),
            ("address", "aa:bb:cc:dd:ee:ff"),
            ("type", "1"),
            ("uevent", "INTERFACE=eth0\nIFINDEX=2"),
        ] {
            write(&eth, f, v);
        }
        let wg = root.path().join("inferos0");
        for (f, v) in [
            ("statistics/rx_bytes", "5"),
            ("operstate", "unknown"),
            ("carrier", "1"),
            ("mtu", "1380"),
            ("speed", "-1"),
            ("type", "65534"),
            ("uevent", "DEVTYPE=wireguard\nINTERFACE=inferos0"),
        ] {
            write(&wg, f, v);
        }
        // Administratively down: carrier is unreadable.
        write(&root.path().join("eno1"), "operstate", "down");
        write(&root.path().join("lo"), "operstate", "unknown");
        write(&root.path().join("veth12ab"), "operstate", "up");
        root
    }

    #[test]
    fn reads_counters_state_and_kind() {
        let root = fake_sysfs();
        let all = read_interfaces(root.path(), &InterfaceFilter::default()).unwrap();
        let names: Vec<_> = all.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["eno1", "eth0", "inferos0"]);

        let eth = &all[1];
        assert_eq!(eth.kind, "ether");
        assert_eq!((eth.rx_bytes, eth.tx_bytes), (Some(1000), Some(2000)));
        assert_eq!((eth.rx_errors, eth.rx_drops), (Some(1), Some(3)));
        assert!(eth.up);
        assert_eq!(eth.speed_bytes, Some(1_250_000_000));

        let wg = &all[2];
        assert_eq!(wg.kind, "wireguard");
        assert!(wg.up, "unknown operstate with carrier counts as up");
        assert_eq!(
            (wg.mtu, wg.speed_bytes, wg.tx_bytes),
            (Some(1380), None, None)
        );

        let down = &all[0];
        assert!(!down.up);
        assert_eq!(down.carrier, None);
    }

    #[test]
    fn include_overrides_the_default_exclusions() {
        let f = InterfaceFilter::new(vec!["veth*".into()]);
        assert!(f.keep("veth12ab") && f.keep("eth0") && !f.keep("lo"));
        let d = InterfaceFilter::default();
        assert!(!d.keep("docker0") && !d.keep("br-1a2b") && d.keep("inferos0") && d.keep("wg0"));
    }

    #[test]
    fn glob_matches_like_a_shell() {
        assert!(glob("veth*", "veth0"));
        assert!(glob("*", ""));
        assert!(glob("e?h*", "eth0"));
        assert!(glob("br-*-x", "br-abc-x"));
        assert!(!glob("veth*", "eth0"));
        assert!(!glob("lo", "lo0"));
    }

    /// A self-deleting temporary directory, so the tests need no extra crate.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};
        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new() -> Self {
                static N: AtomicU32 = AtomicU32::new(0);
                let p = std::env::temp_dir().join(format!(
                    "plows-net-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}

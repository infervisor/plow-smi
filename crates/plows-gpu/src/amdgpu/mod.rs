//! Direct `amdgpu` kernel-driver access: PCI sysfs, hwmon, KFD topology and
//! DRM fdinfo. No ROCm userspace library is loaded, so AMD GPUs work on any
//! Linux host where the `amdgpu` kernel module drives them.
//!
//! Every path is resolved under a host root (`/` by default). Set
//! [`HOST_ROOT_ENV`] to read a host filesystem mounted elsewhere, e.g. `/host`
//! in a container that bind-mounts the host's `/sys` and `/proc`.
//!
//! Units follow the kernel ABI (`Documentation/gpu/amdgpu/thermal.rst`):
//! hwmon power in µW, temperature in m°C, frequency in Hz.

pub(crate) mod pci_ids;
pub(crate) mod procs;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{GpuError, Result};

/// Environment variable naming the host filesystem root (default `/`).
pub const HOST_ROOT_ENV: &str = "PLOWS_HOST_ROOT";

/// Host filesystem root from [`HOST_ROOT_ENV`], or `/`.
pub(crate) fn host_root() -> PathBuf {
    std::env::var_os(HOST_ROOT_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `power_dpm_force_performance_level` values, in AMD SMI's
/// `amdsmi_dev_perf_level_t` order so codes stay stable for callers.
pub(crate) const PERF_LEVELS: [&str; 9] = [
    "auto",
    "low",
    "high",
    "manual",
    "profile_standard",
    "profile_peak",
    "profile_min_mclk",
    "profile_min_sclk",
    "perf_determinism",
];

/// Temperature sensors in preference order (hotspot first, like AMD SMI).
const TEMP_LABELS: [&str; 3] = ["junction", "edge", "mem"];

/// PCI identity read from config-space sysfs attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PciId {
    pub vendor: u16,
    pub device: u16,
}

/// One GPU driven by `amdgpu`, with every sysfs path resolved once.
#[derive(Debug, Clone)]
pub(crate) struct Device {
    /// PCI address, `dddd:bb:dd.f`.
    pub pci_addr: String,
    /// The PCI device directory.
    pub dir: PathBuf,
    pub pci: PciId,
    /// 64-bit chip unique id, when the ASIC exposes one.
    pub unique_id: Option<u64>,
    /// KFD `gpu_id`, used to key per-process KFD counters.
    pub kfd_gpu_id: Option<u32>,
    /// LLVM target, e.g. `gfx1201`.
    pub gfx_target: Option<String>,
    hwmon: Option<PathBuf>,
    temps: Vec<PathBuf>,
    sclk: Option<PathBuf>,
    mclk: Option<PathBuf>,
}

/// One refresh worth of telemetry. `None` means the driver did not report it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Sample {
    pub busy_percent: Option<f32>,
    pub mem_busy_percent: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub temperature_c: Option<f32>,
    pub power_w: Option<f32>,
    pub power_cap_w: Option<f32>,
    pub fan_rpm: Option<f32>,
    pub sclk_mhz: Option<u32>,
    pub mclk_mhz: Option<u32>,
}

/// Power caps in µW.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PowerCapsUw {
    pub current: Option<u64>,
    pub default: Option<u64>,
    pub min: Option<u64>,
    pub max: Option<u64>,
}

/// Every PCI display device bound to `amdgpu`, ordered by PCI address.
pub(crate) fn discover(root: &Path) -> Result<Vec<Device>> {
    let driver_dir = root.join("sys/bus/pci/drivers/amdgpu");
    let entries = fs::read_dir(&driver_dir).map_err(|e| {
        GpuError::NoDevices(format!(
            "amdgpu driver not loaded ({}: {e})",
            driver_dir.display()
        ))
    })?;
    let mut addrs: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| parse_pci_addr(name).is_some())
        .collect();
    addrs.sort();

    let kfd = kfd_nodes(root);
    let mut devices = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let dir = root.join("sys/bus/pci/devices").join(&addr);
        // PCI class 0x03xxxx: VGA (0300) or other display controller (0380).
        if !read_trim(&dir.join("class")).is_some_and(|c| c.starts_with("0x03")) {
            continue;
        }
        devices.push(Device::probe(addr, dir, &kfd));
    }
    if devices.is_empty() {
        return Err(GpuError::NoDevices(
            "amdgpu is loaded but drives no display-class PCI device".into(),
        ));
    }
    Ok(devices)
}

/// `amdgpu` version: the DKMS module version when installed, else the kernel release.
pub(crate) fn driver_version(root: &Path) -> Option<String> {
    read_trim(&root.join("sys/module/amdgpu/version"))
        .or_else(|| read_trim(&root.join("proc/sys/kernel/osrelease")))
}

impl Device {
    fn probe(pci_addr: String, dir: PathBuf, kfd: &HashMap<String, KfdNode>) -> Self {
        let pci = PciId {
            vendor: read_hex(&dir.join("vendor")).unwrap_or(0x1002) as u16,
            device: read_hex(&dir.join("device")).unwrap_or(0) as u16,
        };
        let unique_id = read_trim(&dir.join("unique_id"))
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok());
        let node = kfd.get(&pci_addr);
        let hwmon = find_hwmon(&dir);
        let labelled = |prefix: &str| -> HashMap<String, PathBuf> {
            hwmon
                .as_deref()
                .map(|h| hwmon_inputs(h, prefix))
                .unwrap_or_default()
        };
        let temp_map = labelled("temp");
        let mut temps: Vec<PathBuf> = TEMP_LABELS
            .iter()
            .filter_map(|l| temp_map.get(*l).cloned())
            .collect();
        if temps.is_empty() {
            // Unlabelled sensors (rare): fall back to numeric order.
            let mut rest: Vec<PathBuf> = temp_map.into_values().collect();
            rest.sort();
            temps = rest;
        }
        let freq_map = labelled("freq");
        Self {
            pci_addr,
            pci,
            unique_id,
            kfd_gpu_id: node.map(|n| n.gpu_id),
            gfx_target: node.and_then(|n| n.gfx_target_version).and_then(gfx_target),
            sclk: freq_map.get("sclk").cloned(),
            mclk: freq_map.get("mclk").cloned(),
            temps,
            hwmon,
            dir,
        }
    }

    /// AMD SMI's UUID layout, so series keyed by UUID survive the backend change.
    pub fn uuid(&self) -> String {
        match self.unique_id {
            Some(uid) => amd_uuid(uid, self.pci.device, 0),
            None => format!("amd-gpu-{}", self.pci_addr),
        }
    }

    /// Board product name (FRU EEPROM), then the `pci.ids` device name.
    pub fn model(&self, root: &Path, names: &mut pci_ids::Cache) -> String {
        read_trim(&self.dir.join("product_name"))
            .filter(|s| !s.is_empty())
            .or_else(|| names.device_name(root, self.pci.vendor, self.pci.device))
            .unwrap_or_else(|| format!("AMD GPU [{:04x}:{:04x}]", self.pci.vendor, self.pci.device))
    }

    pub fn serial(&self) -> Option<String> {
        read_trim(&self.dir.join("serial_number")).filter(|s| !s.is_empty())
    }

    pub fn sample(&self) -> Sample {
        let hw = |name: &str| self.hwmon.as_ref().map(|h| h.join(name));
        let read_hw = |name: &str| hw(name).and_then(|p| read_u64(&p));
        let vram_total = read_u64(&self.dir.join("mem_info_vram_total"));
        Sample {
            busy_percent: read_percent(&self.dir.join("gpu_busy_percent")),
            mem_busy_percent: read_percent(&self.dir.join("mem_busy_percent")),
            vram_used: read_u64(&self.dir.join("mem_info_vram_used")),
            vram_total,
            temperature_c: self
                .temps
                .iter()
                .find_map(|p| read_i64(p))
                .map(|mc| mc as f32 / 1000.0),
            // power1_input is instantaneous (newer SMUs); power1_average otherwise.
            power_w: read_hw("power1_input")
                .or_else(|| read_hw("power1_average"))
                .map(uw_to_w),
            power_cap_w: read_hw("power1_cap").filter(|&uw| uw > 0).map(uw_to_w),
            fan_rpm: read_hw("fan1_input").map(|r| r as f32),
            sclk_mhz: self
                .sclk
                .as_ref()
                .and_then(|p| read_u64(p))
                .map(hz_to_mhz)
                .or_else(|| dpm_current_mhz(&self.dir.join("pp_dpm_sclk"))),
            mclk_mhz: self
                .mclk
                .as_ref()
                .and_then(|p| read_u64(p))
                .map(hz_to_mhz)
                .or_else(|| dpm_current_mhz(&self.dir.join("pp_dpm_mclk"))),
        }
    }

    fn power_cap_path(&self) -> Option<PathBuf> {
        self.hwmon.as_ref().map(|h| h.join("power1_cap"))
    }

    /// The power cap attribute exists and is writable by its owner (root).
    pub fn power_cap_writable(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.power_cap_path()
                .and_then(|p| fs::metadata(p).ok())
                .is_some_and(|m| m.permissions().mode() & 0o200 != 0)
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    pub fn power_caps_uw(&self) -> PowerCapsUw {
        let read = |name: &str| self.hwmon.as_ref().and_then(|h| read_u64(&h.join(name)));
        PowerCapsUw {
            current: read("power1_cap").filter(|&v| v > 0),
            default: read("power1_cap_default").filter(|&v| v > 0),
            min: read("power1_cap_min"),
            max: read("power1_cap_max").filter(|&v| v > 0),
        }
    }

    /// Write the power cap. The driver rejects values outside min..=max.
    pub fn set_power_cap_uw(&self, uw: u64) -> Result<()> {
        let path = self
            .power_cap_path()
            .filter(|p| p.exists())
            .ok_or_else(|| GpuError::Unsupported(format!("{}: no power1_cap", self.pci_addr)))?;
        write_attr(&path, &uw.to_string())
    }

    /// Index into [`PERF_LEVELS`].
    pub fn perf_level(&self) -> Option<u32> {
        let s = read_trim(&self.dir.join("power_dpm_force_performance_level"))?;
        PERF_LEVELS.iter().position(|l| *l == s).map(|i| i as u32)
    }

    pub fn set_perf_level(&self, level: &str) -> Result<()> {
        if !PERF_LEVELS.contains(&level) {
            return Err(GpuError::Unsupported(format!(
                "unknown AMD perf level `{level}`"
            )));
        }
        write_attr(&self.dir.join("power_dpm_force_performance_level"), level)
    }
}

/// `amdsmi_get_gpu_device_uuid` layout: unique-id high byte, partition,
/// device id, then the remaining 56 bits of the unique id.
fn amd_uuid(unique_id: u64, device_id: u16, partition: u8) -> String {
    format!(
        "{:02x}{:02x}{:04x}-0000-1000-80{:02x}-{:012x}",
        unique_id >> 56,
        partition,
        device_id,
        (unique_id >> 48) & 0xff,
        unique_id & 0xffff_ffff_ffff
    )
}

/// KFD `gfx_target_version` (`MMmmss`, decimal) → LLVM target (`gfxMMmS`, hex digits).
fn gfx_target(version: u32) -> Option<String> {
    (version > 0).then(|| {
        format!(
            "gfx{}{:x}{:x}",
            version / 10000,
            (version / 100) % 100,
            version % 100
        )
    })
}

struct KfdNode {
    gpu_id: u32,
    gfx_target_version: Option<u32>,
}

/// KFD topology GPU nodes keyed by PCI address.
fn kfd_nodes(root: &Path) -> HashMap<String, KfdNode> {
    let mut out = HashMap::new();
    let Ok(nodes) = fs::read_dir(root.join("sys/class/kfd/kfd/topology/nodes")) else {
        return out;
    };
    for node in nodes.filter_map(|e| e.ok()).map(|e| e.path()) {
        let Some(gpu_id) = read_u64(&node.join("gpu_id")).filter(|&id| id > 0) else {
            continue; // CPU node
        };
        let Some(props) = read_trim(&node.join("properties")) else {
            continue;
        };
        let prop = |key: &str| {
            props.lines().find_map(|l| {
                let (k, v) = l.split_once(' ')?;
                (k == key).then(|| v.trim().parse::<u64>().ok()).flatten()
            })
        };
        let (Some(domain), Some(location)) = (prop("domain"), prop("location_id")) else {
            continue;
        };
        let addr = format!(
            "{:04x}:{:02x}:{:02x}.{:x}",
            domain,
            (location >> 8) & 0xff,
            (location >> 3) & 0x1f,
            location & 0x7
        );
        // Partitioned parts list several nodes per PCI device; keep the first.
        out.entry(addr).or_insert(KfdNode {
            gpu_id: gpu_id as u32,
            gfx_target_version: prop("gfx_target_version").map(|v| v as u32),
        });
    }
    out
}

fn find_hwmon(dir: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir.join("hwmon"))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .find(|h| read_trim(&h.join("name")).as_deref() == Some("amdgpu"))
}

/// `<prefix>N_label` → `<prefix>N_input` for one hwmon directory.
fn hwmon_inputs(hwmon: &Path, prefix: &str) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let Ok(entries) = fs::read_dir(hwmon) else {
        return out;
    };
    for name in entries.filter_map(|e| e.ok()?.file_name().into_string().ok()) {
        let Some(stem) = name.strip_suffix("_input") else {
            continue;
        };
        if !stem
            .strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let label = read_trim(&hwmon.join(format!("{stem}_label"))).unwrap_or_else(|| stem.into());
        out.entry(label).or_insert_with(|| hwmon.join(&name));
    }
    out
}

/// `dddd:bb:dd.f` → (domain, bus, device, function).
pub(crate) fn parse_pci_addr(s: &str) -> Option<(u32, u8, u8, u8)> {
    let (domain, rest) = s.split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (dev, func) = rest.split_once('.')?;
    if domain.len() != 4 || bus.len() != 2 || dev.len() != 2 || func.len() != 1 {
        return None;
    }
    Some((
        u32::from_str_radix(domain, 16).ok()?,
        u8::from_str_radix(bus, 16).ok()?,
        u8::from_str_radix(dev, 16).ok()?,
        u8::from_str_radix(func, 16).ok()?,
    ))
}

/// The current (`*`) level of a `pp_dpm_*` table, e.g. `1: 582Mhz *`.
fn parse_dpm_current(text: &str) -> Option<u32> {
    text.lines()
        .find(|l| l.trim_end().ends_with('*'))
        .and_then(|l| {
            let lower = l.to_ascii_lowercase();
            let end = lower.find("mhz")?;
            let digits = lower[..end].trim_end();
            let start = digits
                .rfind(|c: char| !c.is_ascii_digit())
                .map_or(0, |i| i + 1);
            digits[start..].parse().ok()
        })
}

fn dpm_current_mhz(path: &Path) -> Option<u32> {
    parse_dpm_current(&fs::read_to_string(path).ok()?)
}

fn uw_to_w(uw: u64) -> f32 {
    (uw as f64 / 1e6) as f32
}

fn hz_to_mhz(hz: u64) -> u32 {
    ((hz + 500_000) / 1_000_000) as u32
}

pub(crate) fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub(crate) fn read_u64(path: &Path) -> Option<u64> {
    read_trim(path)?.parse().ok()
}

fn read_i64(path: &Path) -> Option<i64> {
    read_trim(path)?.parse().ok()
}

fn read_hex(path: &Path) -> Option<u64> {
    let s = read_trim(path)?;
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

fn read_percent(path: &Path) -> Option<f32> {
    read_u64(path).map(|v| v.min(100) as f32)
}

/// Write an existing attribute; never creates a file.
fn write_attr(path: &Path, value: &str) -> Result<()> {
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .and_then(|mut f| f.write_all(value.as_bytes()))
        .map_err(|e| {
            GpuError::DeviceQueryFailed(match e.kind() {
                io::ErrorKind::PermissionDenied => {
                    format!("{}: permission denied (requires root)", path.display())
                }
                _ => format!("write `{value}` to {}: {e}", path.display()),
            })
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fake host root laid out like the amdgpu sysfs ABI.
    pub(crate) struct FakeHost {
        pub root: PathBuf,
    }

    impl FakeHost {
        pub fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "plows-amdgpu-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        pub fn write(&self, rel: &str, value: &str) {
            let p = self.root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, value).unwrap();
        }

        pub fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.root.join(rel)).unwrap()
        }

        /// An R9700-like GPU at `addr`, with hwmon and a KFD node.
        pub fn gpu(&self, addr: &str, node: u32, gpu_id: u32, unique_id: &str) {
            fs::create_dir_all(self.root.join("sys/bus/pci/drivers/amdgpu").join(addr)).unwrap();
            let d = format!("sys/bus/pci/devices/{addr}");
            for (f, v) in [
                ("class", "0x030000"),
                ("vendor", "0x1002"),
                ("device", "0x7551"),
                ("unique_id", unique_id),
                ("gpu_busy_percent", "42"),
                ("mem_busy_percent", "7"),
                ("mem_info_vram_used", "59912192"),
                ("mem_info_vram_total", "34208743424"),
                ("power_dpm_force_performance_level", "auto"),
                ("pp_dpm_sclk", "S: 0Mhz *\n1: 500Mhz\n2: 2350Mhz\n"),
                ("pp_dpm_mclk", "0: 96Mhz *\n1: 456Mhz\n"),
            ] {
                self.write(&format!("{d}/{f}"), &format!("{v}\n"));
            }
            let h = format!("{d}/hwmon/hwmon{node}");
            for (f, v) in [
                ("name", "amdgpu"),
                ("temp1_input", "29000"),
                ("temp1_label", "edge"),
                ("temp2_input", "31000"),
                ("temp2_label", "junction"),
                ("temp3_input", "30000"),
                ("temp3_label", "mem"),
                ("power1_average", "13000000"),
                ("power1_cap", "300000000"),
                ("power1_cap_default", "300000000"),
                ("power1_cap_min", "210000000"),
                ("power1_cap_max", "300000000"),
                ("freq1_input", "0"),
                ("freq1_label", "sclk"),
                ("freq2_input", "96000000"),
                ("freq2_label", "mclk"),
            ] {
                self.write(&format!("{h}/{f}"), &format!("{v}\n"));
            }
            let (domain, bus, dev, func) = parse_pci_addr(addr).unwrap();
            let location = (bus as u32) << 8 | (dev as u32) << 3 | func as u32;
            let n = format!("sys/class/kfd/kfd/topology/nodes/{node}");
            self.write(&format!("{n}/gpu_id"), &format!("{gpu_id}\n"));
            self.write(
                &format!("{n}/properties"),
                &format!(
                    "simd_count 128\ngfx_target_version 120001\nlocation_id {location}\ndomain {domain}\n"
                ),
            );
        }
    }

    impl Drop for FakeHost {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn uuid_matches_amd_smi_on_r9700() {
        // Observed on two Radeon AI PRO R9700 boards: sysfs unique_id vs amd-smi UUID.
        assert_eq!(
            amd_uuid(0x2de8238c16749fb8, 0x7551, 0),
            "2d007551-0000-1000-80e8-238c16749fb8"
        );
        assert_eq!(
            amd_uuid(0x1bff72496053c482, 0x7551, 0),
            "1b007551-0000-1000-80ff-72496053c482"
        );
    }

    #[test]
    fn gfx_target_from_kfd_version() {
        assert_eq!(gfx_target(120001).as_deref(), Some("gfx1201"));
        assert_eq!(gfx_target(90402).as_deref(), Some("gfx942"));
        assert_eq!(gfx_target(90010).as_deref(), Some("gfx90a"));
        assert_eq!(gfx_target(100300).as_deref(), Some("gfx1030"));
        assert_eq!(gfx_target(0), None);
    }

    #[test]
    fn dpm_current_level() {
        assert_eq!(
            parse_dpm_current("0: 313Mhz\n1: 582Mhz *\n2: 2400Mhz\n"),
            Some(582)
        );
        assert_eq!(parse_dpm_current("S: 0Mhz *\n1: 500Mhz\n"), Some(0));
        assert_eq!(parse_dpm_current("0: 100MHz\n1: 200MHz\n"), None);
    }

    #[test]
    fn pci_addresses() {
        assert_eq!(parse_pci_addr("0000:43:00.0"), Some((0, 0x43, 0, 0)));
        assert_eq!(parse_pci_addr("0001:c1:1f.7"), Some((1, 0xc1, 0x1f, 7)));
        assert_eq!(parse_pci_addr("new_id"), None);
        assert_eq!(parse_pci_addr("module"), None);
    }

    #[test]
    fn discovers_and_samples_in_pci_order() {
        let host = FakeHost::new("discover");
        host.gpu("0000:43:00.0", 2, 8792, "1bff72496053c482");
        host.gpu("0000:03:00.0", 1, 53865, "2de8238c16749fb8");
        // Audio function and driver control files are not GPUs.
        fs::create_dir_all(host.root.join("sys/bus/pci/drivers/amdgpu/module")).unwrap();
        fs::create_dir_all(host.root.join("sys/bus/pci/drivers/amdgpu/0000:03:00.1")).unwrap();
        host.write("sys/bus/pci/devices/0000:03:00.1/class", "0x040300\n");

        let devs = discover(&host.root).unwrap();
        assert_eq!(
            devs.iter().map(|d| d.pci_addr.as_str()).collect::<Vec<_>>(),
            ["0000:03:00.0", "0000:43:00.0"]
        );
        let d = &devs[0];
        assert_eq!(d.kfd_gpu_id, Some(53865));
        assert_eq!(d.gfx_target.as_deref(), Some("gfx1201"));
        assert_eq!(d.uuid(), "2d007551-0000-1000-80e8-238c16749fb8");
        assert_eq!(
            d.model(&host.root, &mut pci_ids::Cache::default()),
            "AMD GPU [1002:7551]"
        );
        assert_eq!(
            d.sample(),
            Sample {
                busy_percent: Some(42.0),
                mem_busy_percent: Some(7.0),
                vram_used: Some(59912192),
                vram_total: Some(34208743424),
                temperature_c: Some(31.0),
                power_w: Some(13.0),
                power_cap_w: Some(300.0),
                fan_rpm: None,
                sclk_mhz: Some(0),
                mclk_mhz: Some(96),
            }
        );
    }

    #[test]
    fn falls_back_without_hwmon_labels_and_prefers_power_input() {
        let host = FakeHost::new("fallback");
        host.gpu("0000:03:00.0", 1, 1, "2de8238c16749fb8");
        let h = "sys/bus/pci/devices/0000:03:00.0/hwmon/hwmon1";
        for f in [
            "freq1_input",
            "freq1_label",
            "freq2_input",
            "freq2_label",
            "temp2_label",
        ] {
            fs::remove_file(host.root.join(h).join(f)).unwrap();
        }
        host.write(&format!("{h}/power1_input"), "250500000\n");
        let s = discover(&host.root).unwrap()[0].sample();
        assert_eq!(s.power_w, Some(250.5));
        assert_eq!(s.temperature_c, Some(29.0)); // edge, since junction lost its label
        assert_eq!((s.sclk_mhz, s.mclk_mhz), (Some(0), Some(96))); // pp_dpm tables
    }

    #[test]
    fn missing_driver_is_no_devices() {
        let host = FakeHost::new("none");
        assert!(matches!(discover(&host.root), Err(GpuError::NoDevices(_))));
        fs::create_dir_all(host.root.join("sys/bus/pci/drivers/amdgpu")).unwrap();
        assert!(matches!(discover(&host.root), Err(GpuError::NoDevices(_))));
    }

    #[test]
    fn control_writes_sysfs() {
        let host = FakeHost::new("control");
        host.gpu("0000:03:00.0", 1, 1, "2de8238c16749fb8");
        let d = &discover(&host.root).unwrap()[0];
        assert_eq!(d.perf_level(), Some(0));
        d.set_perf_level("high").unwrap();
        assert_eq!(d.perf_level(), Some(2));
        assert!(d.set_perf_level("turbo").is_err());
        assert_eq!(
            d.power_caps_uw(),
            PowerCapsUw {
                current: Some(300_000_000),
                default: Some(300_000_000),
                min: Some(210_000_000),
                max: Some(300_000_000),
            }
        );
        d.set_power_cap_uw(250_000_000).unwrap();
        assert_eq!(
            host.read("sys/bus/pci/devices/0000:03:00.0/hwmon/hwmon1/power1_cap"),
            "250000000"
        );
    }

    #[test]
    fn driver_version_prefers_module_version() {
        let host = FakeHost::new("version");
        assert_eq!(driver_version(&host.root), None);
        host.write("proc/sys/kernel/osrelease", "7.0.0-30-generic\n");
        assert_eq!(
            driver_version(&host.root).as_deref(),
            Some("7.0.0-30-generic")
        );
        host.write("sys/module/amdgpu/version", "6.12.12\n");
        assert_eq!(driver_version(&host.root).as_deref(), Some("6.12.12"));
    }
}

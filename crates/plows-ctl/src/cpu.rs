//! CPU frequency control through Linux cpufreq sysfs.
//!
//! Every path is built from a root (`/` on a real machine), so the module is
//! tested against a temporary directory laid out like sysfs — no root and no
//! real hardware needed.
//!
//! RAPL package power limits are **read only** here. They interact with
//! firmware and platform settings in ways that deserve their own step.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What cpufreq reports for this machine.
///
/// A value that differs between policies is reported as `None`, never as the
/// first policy's value: a setting that is only half in effect is not in
/// effect, and comparing against it would skip a write that is needed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuCaps {
    /// `scaling_driver`: `intel_pstate`, `amd-pstate`, `acpi-cpufreq`, …
    pub driver: Option<String>,
    pub policies: usize,
    pub available_governors: Vec<String>,
    pub governor: Option<String>,
    /// The range `scaling_max_freq` may be set within, in kHz.
    pub hw_min_khz: Option<u64>,
    pub hw_max_khz: Option<u64>,
    /// `scaling_max_freq` now.
    pub max_freq_khz: Option<u64>,
    /// `scaling_available_frequencies`, highest first. Empty for drivers that
    /// take any value in the range.
    pub available_frequencies_khz: Vec<u64>,
    /// `energy_performance_preference`, where the driver exposes it.
    pub epp: Option<String>,
    pub available_epp: Vec<String>,
    /// RAPL package power limits, read only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rapl: Vec<RaplZone>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaplZone {
    pub name: String,
    pub power_limit_uw: Option<u64>,
}

/// cpufreq under a sysfs root.
#[derive(Debug, Clone)]
pub struct CpuFreq {
    root: PathBuf,
}

impl Default for CpuFreq {
    fn default() -> Self {
        Self::system()
    }
}

impl CpuFreq {
    /// The real machine.
    pub fn system() -> Self {
        Self::new("/")
    }

    /// A tree rooted somewhere else — used by tests.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn cpufreq_dir(&self) -> PathBuf {
        self.root.join("sys/devices/system/cpu/cpufreq")
    }

    /// Policy directories, in numeric order.
    fn policies(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.cpufreq_dir()) else {
            return Vec::new();
        };
        let mut out: Vec<(u32, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let n = name.strip_prefix("policy")?.parse::<u32>().ok()?;
                Some((n, e.path()))
            })
            .collect();
        out.sort_by_key(|(n, _)| *n);
        out.into_iter().map(|(_, p)| p).collect()
    }

    /// Read everything. `None` when the machine has no cpufreq at all.
    pub fn read(&self) -> Option<CpuCaps> {
        let policies = self.policies();
        policies.first()?;
        let mut available_frequencies_khz: Vec<u64> =
            common_words(&policies, "scaling_available_frequencies")
                .iter()
                .filter_map(|w| w.parse().ok())
                .collect();
        available_frequencies_khz.sort_unstable_by(|a, b| b.cmp(a));
        available_frequencies_khz.dedup();
        let ranges: Option<Vec<(u64, u64)>> = policies
            .iter()
            .map(|p| {
                let steps: Vec<u64> = words(p, "scaling_available_frequencies")
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                Some((
                    steps
                        .iter()
                        .min()
                        .copied()
                        .or_else(|| num(p, "cpuinfo_min_freq"))?,
                    steps
                        .iter()
                        .max()
                        .copied()
                        .or_else(|| num(p, "cpuinfo_max_freq"))?,
                ))
            })
            .collect();
        let range = ranges.and_then(|r| {
            let lo = r.iter().map(|r| r.0).max()?;
            let hi = r.iter().map(|r| r.1).min()?;
            (lo <= hi).then_some((lo, hi))
        });
        let (hw_min_khz, hw_max_khz) = range.map_or((None, None), |(lo, hi)| (Some(lo), Some(hi)));

        Some(CpuCaps {
            driver: uniform(&policies, |p| text(p, "scaling_driver")),
            policies: policies.len(),
            available_governors: common_words(&policies, "scaling_available_governors"),
            governor: uniform(&policies, |p| text(p, "scaling_governor")),
            hw_min_khz,
            hw_max_khz,
            max_freq_khz: uniform(&policies, |p| num(p, "scaling_max_freq")),
            available_frequencies_khz,
            epp: uniform(&policies, |p| text(p, "energy_performance_preference")),
            available_epp: common_words(&policies, "energy_performance_available_preferences"),
            rapl: self.rapl(),
        })
    }

    fn rapl(&self) -> Vec<RaplZone> {
        let dir = self.root.join("sys/class/powercap");
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut zones: Vec<RaplZone> = entries
            .flatten()
            .filter(|e| {
                // Top-level package zones only: `intel-rapl:0`, not the
                // `intel-rapl` control type or `intel-rapl:0:0` subzones.
                let n = e.file_name().to_string_lossy().into_owned();
                n.matches(':').count() == 1
            })
            .map(|e| {
                let p = e.path();
                RaplZone {
                    name: text(&p, "name")
                        .unwrap_or_else(|| e.file_name().to_string_lossy().into()),
                    power_limit_uw: num(&p, "constraint_0_power_limit_uw"),
                }
            })
            .collect();
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        zones
    }

    /// Write `scaling_governor` on every policy.
    pub fn set_governor(&self, governor: &str) -> io::Result<()> {
        self.write_all("scaling_governor", governor)
    }

    /// Write `scaling_max_freq` (kHz) on every policy.
    pub fn set_max_freq_khz(&self, khz: u64) -> io::Result<()> {
        self.write_all("scaling_max_freq", &khz.to_string())
    }

    /// Write `energy_performance_preference` on every policy.
    pub fn set_epp(&self, epp: &str) -> io::Result<()> {
        self.write_all("energy_performance_preference", epp)
    }

    fn write_all(&self, file: &str, value: &str) -> io::Result<()> {
        let policies = self.policies();
        if policies.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no cpufreq policies",
            ));
        }
        // Keep going after a failure, so one offline CPU does not leave the
        // rest unchanged; report the first error.
        let mut first_err = None;
        for p in &policies {
            if let Err(e) = fs::write(p.join(file), value) {
                first_err.get_or_insert(io::Error::new(
                    e.kind(),
                    format!("{}: {e}", p.join(file).display()),
                ));
            }
        }
        first_err.map_or(Ok(()), Err)
    }
}

fn text(dir: &Path, file: &str) -> Option<String> {
    let s = fs::read_to_string(dir.join(file)).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn num(dir: &Path, file: &str) -> Option<u64> {
    text(dir, file)?.parse().ok()
}

fn words(dir: &Path, file: &str) -> Vec<String> {
    text(dir, file)
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn common_words(policies: &[PathBuf], file: &str) -> Vec<String> {
    let Some(first) = policies.first() else {
        return vec![];
    };
    let mut common = words(first, file);
    for policy in &policies[1..] {
        let available = words(policy, file);
        common.retain(|value| available.contains(value));
    }
    common
}

/// The value every policy agrees on, or `None`.
fn uniform<T: PartialEq>(policies: &[PathBuf], read: impl Fn(&Path) -> Option<T>) -> Option<T> {
    let mut it = policies.iter().map(|p| read(p));
    let first = it.next()??;
    it.all(|v| v.as_ref() == Some(&first)).then_some(first)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Lay out a fake sysfs tree.
    pub(crate) struct FakeSys {
        pub dir: PathBuf,
    }

    impl FakeSys {
        pub fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "plows-ctl-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        pub fn policy(&self, n: u32, files: &[(&str, &str)]) -> &Self {
            let p = self
                .dir
                .join(format!("sys/devices/system/cpu/cpufreq/policy{n}"));
            fs::create_dir_all(&p).unwrap();
            for (f, v) in files {
                fs::write(p.join(f), format!("{v}\n")).unwrap();
            }
            self
        }

        pub fn read(&self, n: u32, file: &str) -> String {
            fs::read_to_string(
                self.dir
                    .join(format!("sys/devices/system/cpu/cpufreq/policy{n}/{file}")),
            )
            .unwrap()
            .trim()
            .to_string()
        }

        pub fn cpu(&self) -> CpuFreq {
            CpuFreq::new(&self.dir)
        }
    }

    impl Drop for FakeSys {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    pub(crate) fn acpi(n: u32, gov: &str, max: &str) -> Vec<(&'static str, String)> {
        let _ = n;
        vec![
            ("scaling_driver", "acpi-cpufreq".into()),
            (
                "scaling_available_governors",
                "conservative ondemand userspace powersave performance schedutil".into(),
            ),
            ("scaling_governor", gov.into()),
            (
                "scaling_available_frequencies",
                "3300000 2400000 1500000".into(),
            ),
            ("cpuinfo_min_freq", "1500000".into()),
            ("cpuinfo_max_freq", "5008007".into()),
            ("scaling_max_freq", max.into()),
        ]
    }

    fn put(sys: &FakeSys, n: u32, files: Vec<(&'static str, String)>) {
        let v: Vec<(&str, &str)> = files.iter().map(|(a, b)| (*a, b.as_str())).collect();
        sys.policy(n, &v);
    }

    #[test]
    fn no_cpufreq_is_none() {
        let sys = FakeSys::new("none");
        assert!(sys.cpu().read().is_none());
    }

    #[test]
    fn reads_acpi_cpufreq_and_bounds_the_range_by_the_steps() {
        let sys = FakeSys::new("acpi");
        put(&sys, 0, acpi(0, "schedutil", "3300000"));
        put(&sys, 1, acpi(1, "schedutil", "3300000"));
        let c = sys.cpu().read().unwrap();
        assert_eq!(c.driver.as_deref(), Some("acpi-cpufreq"));
        assert_eq!(c.policies, 2);
        assert_eq!(c.governor.as_deref(), Some("schedutil"));
        // Not the 5 GHz boost ceiling: it cannot be set as a max here.
        assert_eq!(c.hw_max_khz, Some(3_300_000));
        assert_eq!(c.hw_min_khz, Some(1_500_000));
        assert_eq!(
            c.available_frequencies_khz,
            vec![3_300_000, 2_400_000, 1_500_000]
        );
        assert!(c.epp.is_none() && c.available_epp.is_empty());
    }

    #[test]
    fn a_value_policies_disagree_on_is_none() {
        let sys = FakeSys::new("mixed");
        put(&sys, 0, acpi(0, "schedutil", "3300000"));
        put(&sys, 1, acpi(1, "performance", "2400000"));
        let c = sys.cpu().read().unwrap();
        assert_eq!(c.governor, None);
        assert_eq!(c.max_freq_khz, None);
    }

    #[test]
    fn reads_intel_pstate_epp_and_uses_cpuinfo_for_the_range() {
        let sys = FakeSys::new("pstate");
        sys.policy(
            0,
            &[
                ("scaling_driver", "intel_pstate"),
                ("scaling_available_governors", "performance powersave"),
                ("scaling_governor", "powersave"),
                ("cpuinfo_min_freq", "800000"),
                ("cpuinfo_max_freq", "4000000"),
                ("scaling_max_freq", "4000000"),
                ("energy_performance_preference", "balance_performance"),
                (
                    "energy_performance_available_preferences",
                    "default performance balance_performance balance_power power",
                ),
            ],
        );
        let c = sys.cpu().read().unwrap();
        assert_eq!(c.hw_min_khz, Some(800_000));
        assert_eq!(c.hw_max_khz, Some(4_000_000));
        assert_eq!(c.epp.as_deref(), Some("balance_performance"));
        assert_eq!(c.available_epp.len(), 5);
    }

    #[test]
    fn writes_reach_every_policy() {
        let sys = FakeSys::new("write");
        put(&sys, 0, acpi(0, "schedutil", "3300000"));
        put(&sys, 7, acpi(7, "schedutil", "3300000"));
        let cpu = sys.cpu();
        cpu.set_governor("ondemand").unwrap();
        cpu.set_max_freq_khz(2_400_000).unwrap();
        assert_eq!(sys.read(0, "scaling_governor"), "ondemand");
        assert_eq!(sys.read(7, "scaling_governor"), "ondemand");
        assert_eq!(sys.read(7, "scaling_max_freq"), "2400000");
    }

    #[test]
    fn rapl_reads_package_zones_only() {
        let sys = FakeSys::new("rapl");
        for (zone, name, lim) in [
            ("intel-rapl:0", "package-0", "280000000"),
            ("intel-rapl:0:0", "core", "0"),
            ("intel-rapl:1", "package-1", "280000000"),
        ] {
            let p = sys.dir.join("sys/class/powercap").join(zone);
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("name"), name).unwrap();
            fs::write(p.join("constraint_0_power_limit_uw"), lim).unwrap();
        }
        fs::create_dir_all(sys.dir.join("sys/class/powercap/intel-rapl")).unwrap();
        put(&sys, 0, acpi(0, "schedutil", "3300000"));
        let c = sys.cpu().read().unwrap();
        assert_eq!(
            c.rapl,
            vec![
                RaplZone {
                    name: "package-0".into(),
                    power_limit_uw: Some(280_000_000)
                },
                RaplZone {
                    name: "package-1".into(),
                    power_limit_uw: Some(280_000_000)
                },
            ]
        );
    }
}

#[cfg(test)]
mod heterogeneous_tests {
    use super::tests::FakeSys;
    #[test]
    fn capabilities_intersect_every_policy() {
        let sys = FakeSys::new("heterogeneous");
        sys.policy(
            0,
            &[
                ("cpuinfo_min_freq", "800000"),
                ("cpuinfo_max_freq", "5000000"),
                ("scaling_available_governors", "performance powersave"),
                (
                    "energy_performance_available_preferences",
                    "power performance",
                ),
            ],
        );
        sys.policy(
            1,
            &[
                ("cpuinfo_min_freq", "1000000"),
                ("cpuinfo_max_freq", "3000000"),
                ("scaling_available_governors", "powersave"),
                ("energy_performance_available_preferences", "power"),
            ],
        );
        let caps = sys.cpu().read().unwrap();
        assert_eq!(
            (caps.hw_min_khz, caps.hw_max_khz),
            (Some(1000000), Some(3000000))
        );
        assert_eq!(caps.available_governors, ["powersave"]);
        assert_eq!(caps.available_epp, ["power"]);
    }
}

//! Per-process GPU usage from the driver.
//!
//! - KFD (`/sys/class/kfd/kfd/proc/<pid>/vram_<gpu_id>`): every ROCm/HIP
//!   compute process, world-readable, so other users' jobs are visible too.
//! - DRM fdinfo (`/proc/<pid>/fdinfo/<fd>` for `/dev/dri/*` fds): graphics
//!   and other DRM clients, visible for processes the caller may inspect.
//!
//! The ROCm runtime opens every GPU's render node, leaving a few KiB on GPUs
//! a process never uses. A KFD process therefore counts on a GPU only with KFD
//! VRAM or engine time there; a DRM-only client counts with VRAM or engine time.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use super::{read_u64, Device};
use crate::process::GpuProcessInfo;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Usage {
    kfd_vram: u64,
    drm_vram: u64,
    gfx_ns: u64,
    compute_ns: u64,
    any_engine_active: bool,
}

/// One amdgpu DRM client, parsed from fdinfo.
#[derive(Debug, Default, PartialEq, Eq)]
struct DrmClient {
    pdev: String,
    client_id: u64,
    vram_bytes: u64,
    gfx_ns: u64,
    compute_ns: u64,
    any_engine_active: bool,
}

/// Processes using any of `devices`; `gpu_index` is the position in `devices`.
pub(crate) fn collect(root: &Path, devices: &[Device]) -> Vec<GpuProcessInfo> {
    let mut usage: HashMap<(u32, usize), Usage> = HashMap::new();
    let mut kfd_pids = HashSet::new();
    scan_kfd(root, devices, &mut usage, &mut kfd_pids);
    scan_drm(root, devices, &mut usage);

    let mut out: Vec<GpuProcessInfo> = usage
        .into_iter()
        .filter_map(|((pid, idx), u)| {
            let kfd = kfd_pids.contains(&pid);
            let counted = u.kfd_vram > 0 || u.any_engine_active || (!kfd && u.drm_vram > 0);
            if !counted {
                return None;
            }
            let compute = kfd || u.compute_ns > 0;
            let graphics = u.gfx_ns > 0 || !compute;
            Some(GpuProcessInfo {
                pid,
                gpu_index: idx as u32,
                gpu_memory_bytes: u.kfd_vram.max(u.drm_vram),
                process_type: match (compute, graphics) {
                    (true, true) => "C+G",
                    (true, false) => "C",
                    _ => "G",
                }
                .to_string(),
            })
        })
        .collect();
    out.sort_by_key(|p| (p.gpu_index, p.pid));
    out
}

fn scan_kfd(
    root: &Path,
    devices: &[Device],
    usage: &mut HashMap<(u32, usize), Usage>,
    kfd_pids: &mut HashSet<u32>,
) {
    let dir = root.join("sys/class/kfd/kfd/proc");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        kfd_pids.insert(pid);
        for (idx, dev) in devices.iter().enumerate() {
            for gpu_id in &dev.kfd_gpu_ids {
                if let Some(vram) = read_u64(&entry.path().join(format!("vram_{gpu_id}"))) {
                    let u = usage.entry((pid, idx)).or_default();
                    u.kfd_vram = u.kfd_vram.saturating_add(vram);
                }
            }
        }
    }
}

fn scan_drm(root: &Path, devices: &[Device], usage: &mut HashMap<(u32, usize), Usage>) {
    let by_addr: HashMap<&str, usize> = devices
        .iter()
        .enumerate()
        .map(|(i, d)| (d.pci_addr.as_str(), i))
        .collect();
    let Ok(procs) = fs::read_dir(root.join("proc")) else {
        return;
    };
    for entry in procs.filter_map(|e| e.ok()) {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        // Other users' fd tables are unreadable without privileges; skip quietly.
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        let mut seen = HashSet::new();
        for fd in fds.filter_map(|e| e.ok()) {
            let is_dri = fs::read_link(fd.path()).is_ok_and(|t| t.starts_with("/dev/dri/"));
            if !is_dri {
                continue;
            }
            let info = entry.path().join("fdinfo").join(fd.file_name());
            let Some(client) = fs::read_to_string(info).ok().and_then(|t| parse_fdinfo(&t)) else {
                continue;
            };
            let Some(&idx) = by_addr.get(client.pdev.as_str()) else {
                continue;
            };
            // Dup'd fds share one client; count it once.
            if !seen.insert((idx, client.client_id)) {
                continue;
            }
            let u = usage.entry((pid, idx)).or_default();
            u.drm_vram += client.vram_bytes;
            u.gfx_ns += client.gfx_ns;
            u.compute_ns += client.compute_ns;
            u.any_engine_active |= client.any_engine_active;
        }
    }
}

/// Parse an amdgpu DRM fdinfo block (`Documentation/gpu/drm-usage-stats.rst`).
fn parse_fdinfo(text: &str) -> Option<DrmClient> {
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            fields.insert(k.trim(), v.trim());
        }
    }
    if fields.get("drm-driver") != Some(&"amdgpu") {
        return None;
    }
    let bytes = |key: &str| fields.get(key).and_then(|v| parse_size(v));
    let ns = |key: &str| {
        fields
            .get(key)
            .and_then(|v| parse_engine_ns(v))
            .unwrap_or(0)
    };
    Some(DrmClient {
        pdev: fields.get("drm-pdev")?.to_string(),
        client_id: fields.get("drm-client-id")?.parse().ok()?,
        // Older kernels print only drm-memory-*; newer add drm-resident-*.
        vram_bytes: bytes("drm-resident-vram")
            .or_else(|| bytes("drm-memory-vram"))
            .unwrap_or(0),
        gfx_ns: ns("drm-engine-gfx"),
        compute_ns: ns("drm-engine-compute"),
        // Include video, JPEG, DMA and future engines, not just graphics/compute.
        // Require a time unit so drm-engine-capacity-* metadata isn't activity.
        any_engine_active: fields.iter().any(|(key, value)| {
            key.starts_with("drm-engine-") && parse_engine_ns(value).is_some_and(|ns| ns > 0)
        }),
    })
}

fn parse_engine_ns(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let ns = parts.next()?.parse().ok()?;
    (parts.next() == Some("ns") && parts.next().is_none()).then_some(ns)
}

/// `<n>`, `<n> KiB` or `<n> MiB`.
fn parse_size(v: &str) -> Option<u64> {
    let mut it = v.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    let scale = match it.next() {
        None => 1,
        Some("KiB") => 1 << 10,
        Some("MiB") => 1 << 20,
        Some("GiB") => 1 << 30,
        Some(_) => return None,
    };
    Some(n.saturating_mul(scale))
}

#[cfg(test)]
mod tests {
    use super::super::discover;
    use super::super::tests::FakeHost;
    use super::*;

    // Trimmed from a HIP process on kernel 7.0 (two R9700s).
    const FDINFO: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t41\nino:\t2026\n\
drm-driver:\tamdgpu\ndrm-client-id:\t14165\ndrm-pdev:\t0000:43:00.0\npasid:\t14158\n\
drm-total-vram:\t1192088 KiB\ndrm-resident-vram:\t1192088 KiB\n\
drm-memory-vram:\t1192088 KiB\ndrm-memory-gtt: \t8240 KiB\n\
drm-engine-compute:\t123456 ns\n";

    #[test]
    fn parses_amdgpu_fdinfo() {
        assert_eq!(
            parse_fdinfo(FDINFO),
            Some(DrmClient {
                pdev: "0000:43:00.0".into(),
                client_id: 14165,
                vram_bytes: 1192088 * 1024,
                gfx_ns: 0,
                compute_ns: 123456,
                any_engine_active: true,
            })
        );
        assert_eq!(parse_fdinfo("drm-driver:\ti915\ndrm-client-id:\t1\n"), None);
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t0\n"), None);
        assert_eq!(parse_size("56 KiB"), Some(56 * 1024));
        assert_eq!(parse_size("3 MiB"), Some(3 << 20));
        assert_eq!(parse_size("0"), Some(0));
        assert_eq!(parse_size("1 furlong"), None);
    }

    #[cfg(unix)]
    #[test]
    fn aggregates_all_kfd_partitions_without_proc_access() {
        let host = FakeHost::new("partition-procs");
        let addr = "0000:03:00.0";
        // One physical device, two partitions; a repeated ID must not double count.
        host.gpu(addr, 2, 200, "2de8238c16749fb8");
        host.gpu(addr, 1, 100, "2de8238c16749fb8");
        host.gpu(addr, 3, 200, "2de8238c16749fb8");
        let devices = discover(&host.root).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].kfd_gpu_ids, [100, 200]);

        host.write("sys/class/kfd/kfd/proc/42/vram_100", "0\n");
        host.write("sys/class/kfd/kfd/proc/42/vram_200", "1073741824\n");
        host.write("sys/class/kfd/kfd/proc/43/vram_100", "2147483648\n");
        host.write("sys/class/kfd/kfd/proc/43/vram_200", "3221225472\n");
        // Models jobs whose /proc fd tables aren't readable by the observer.
        assert!(!host.root.join("proc").exists());
        let procs = collect(&host.root, &devices);
        assert_eq!(procs.len(), 2);
        assert_eq!((procs[0].pid, procs[0].gpu_memory_bytes), (42, 1 << 30));
        assert_eq!((procs[1].pid, procs[1].gpu_memory_bytes), (43, 5 << 30));
        assert!(procs
            .iter()
            .all(|p| p.gpu_index == 0 && p.process_type == "C"));
    }

    #[cfg(unix)]
    #[test]
    fn includes_video_and_dma_clients_without_resident_vram() {
        use std::os::unix::fs::symlink;
        let host = FakeHost::new("video-procs");
        host.gpu("0000:03:00.0", 1, 100, "2de8238c16749fb8");
        let devices = discover(&host.root).unwrap();
        let engines = ["dec", "enc", "enc_1", "jpeg", "dma", "future_engine"];
        for (i, engine) in engines.iter().enumerate() {
            let pid = 100 + i;
            let dir = host.root.join(format!("proc/{pid}/fd"));
            fs::create_dir_all(&dir).unwrap();
            symlink("/dev/dri/renderD130", dir.join("5")).unwrap();
            host.write(
                &format!("proc/{pid}/fdinfo/5"),
                &format!(
                    "drm-driver:\tamdgpu\ndrm-client-id:\t7\ndrm-pdev:\t0000:03:00.0\n\
                     drm-resident-vram:\t0 KiB\ndrm-memory-gtt:\t32768 KiB\n\
                     drm-engine-{engine}:\t1000000000 ns\n"
                ),
            );
        }
        let procs = collect(&host.root, &devices);
        assert_eq!(procs.len(), engines.len());
        for (i, p) in procs.iter().enumerate() {
            assert_eq!(p.pid, (100 + i) as u32);
            assert_eq!(p.gpu_index, 0);
            assert_eq!(p.gpu_memory_bytes, 0); // Don't mislabel GTT as VRAM.
            assert_eq!(p.process_type, "G");
        }
    }

    #[test]
    fn engine_metadata_and_zero_counters_are_not_activity() {
        let base = "drm-driver:\tamdgpu\ndrm-client-id:\t7\ndrm-pdev:\t0000:03:00.0\n";
        for counter in [
            "drm-engine-dec:\t0 ns\n",
            "drm-engine-capacity-compute:\t4\n",
            "drm-engine-enc:\tmalformed ns\n",
            "drm-engine-dma:\t100 cycles\n",
        ] {
            assert!(
                !parse_fdinfo(&format!("{base}{counter}"))
                    .unwrap()
                    .any_engine_active
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn merges_kfd_and_drm_and_drops_runtime_noise() {
        use std::os::unix::fs::symlink;
        let host = FakeHost::new("procs");
        host.gpu("0000:03:00.0", 1, 53865, "2de8238c16749fb8");
        host.gpu("0000:43:00.0", 2, 8792, "1bff72496053c482");
        let devices = discover(&host.root).unwrap();

        // HIP job (pid 100): KFD VRAM on GPU 1, 56 KiB runtime noise on GPU 0.
        host.write("sys/class/kfd/kfd/proc/100/vram_53865", "0\n");
        host.write("sys/class/kfd/kfd/proc/100/vram_8792", "1282134016\n");
        let fdinfo = |pdev: &str, id: u32, vram_kib: u64, extra: &str| {
            format!(
                "drm-driver:\tamdgpu\ndrm-client-id:\t{id}\ndrm-pdev:\t{pdev}\n\
                 drm-memory-vram:\t{vram_kib} KiB\n{extra}"
            )
        };
        let fd = |pid: u32, n: u32, target: &str, info: &str| {
            let dir = host.root.join(format!("proc/{pid}"));
            fs::create_dir_all(dir.join("fd")).unwrap();
            symlink(target, dir.join(format!("fd/{n}"))).unwrap();
            host.write(&format!("proc/{pid}/fdinfo/{n}"), info);
        };
        fd(
            100,
            7,
            "/dev/dri/renderD130",
            &fdinfo("0000:03:00.0", 1, 56, ""),
        );
        fd(
            100,
            8,
            "/dev/dri/renderD131",
            &fdinfo("0000:43:00.0", 2, 1192088, ""),
        );
        // A compositor (pid 200) on GPU 0, with a dup'd fd of the same client.
        let gfx = fdinfo("0000:03:00.0", 3, 2048, "drm-engine-gfx:\t5000 ns\n");
        fd(200, 4, "/dev/dri/card3", &gfx);
        fd(200, 5, "/dev/dri/card3", &gfx);
        // Non-DRM fds are never read.
        fd(200, 6, "/dev/null", &fdinfo("0000:03:00.0", 9, 999999, ""));

        let procs = collect(&host.root, &devices);
        assert_eq!(
            procs,
            vec![
                GpuProcessInfo {
                    pid: 200,
                    gpu_index: 0,
                    gpu_memory_bytes: 2048 * 1024,
                    process_type: "G".into(),
                },
                GpuProcessInfo {
                    pid: 100,
                    gpu_index: 1,
                    gpu_memory_bytes: 1282134016,
                    process_type: "C".into(),
                },
            ]
        );
    }
}

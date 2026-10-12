//! Unit tests for discovery, error paths, and multi-backend management.
//!
//! Real GPU libraries are optional — these tests use stub [`GpuBackend`]
//! implementations so CI without NVIDIA/AMD/Intel stacks stays green.

use plows_gpu::device::{GpuDevice, Vendor};
use plows_gpu::error::GpuError;
use plows_gpu::ffi::dynlib;
use plows_gpu::metrics::{DeviceMetrics, GpuBackend};
use plows_gpu::GpuManager;

/// Minimal stub backend for manager tests.
struct StubBackend {
    vendor: Vendor,
    devices: Vec<GpuDevice>,
    metrics: Vec<DeviceMetrics>,
}

#[test]
fn filtered_discovery_probes_nothing_when_disabled() {
    let manager = GpuManager::discover_filtered(|_| false);
    assert_eq!(manager.backend_count(), 0);
}

impl StubBackend {
    fn new(vendor: Vendor, n: usize) -> Self {
        let devices = (0..n)
            .map(|i| {
                GpuDevice::new(
                    vendor,
                    i,
                    format!("{vendor}-uuid-{i}"),
                    format!("{vendor} GPU {i}"),
                )
            })
            .collect();
        let metrics = (0..n)
            .map(|i| DeviceMetrics {
                utilization: Some(10.0 * (i as f32 + 1.0)),
                memory_utilization: Some(20.0),
                memory_used: Some(1_000_000 * (i as u64 + 1)),
                memory_total: Some(8_000_000_000),
                temperature: Some(40.0 + i as f32),
                power_usage: Some(50.0),
                power_limit: Some(100.0),
                fan_speed: Some(30.0),
                clock_graphics: Some(1500),
                clock_memory: Some(7000),
                ..Default::default()
            })
            .collect();
        Self {
            vendor,
            devices,
            metrics,
        }
    }
}

impl GpuBackend for StubBackend {
    fn vendor(&self) -> Vendor {
        self.vendor
    }

    fn device_count(&self) -> usize {
        self.devices.len()
    }

    fn devices(&self) -> Vec<GpuDevice> {
        self.devices.clone()
    }

    fn refresh(&mut self) {}

    fn utilization(&self, id: usize) -> Option<f32> {
        self.metrics.get(id).and_then(|m| m.utilization)
    }

    fn memory_used(&self, id: usize) -> Option<u64> {
        self.metrics.get(id).and_then(|m| m.memory_used)
    }

    fn memory_total(&self, id: usize) -> Option<u64> {
        self.metrics.get(id).and_then(|m| m.memory_total)
    }

    fn temperature(&self, id: usize) -> Option<f32> {
        self.metrics.get(id).and_then(|m| m.temperature)
    }

    fn power_usage(&self, id: usize) -> Option<f32> {
        self.metrics.get(id).and_then(|m| m.power_usage)
    }

    fn fan_speed(&self, id: usize) -> Option<f32> {
        self.metrics.get(id).and_then(|m| m.fan_speed)
    }

    fn clock_graphics(&self, id: usize) -> Option<u32> {
        self.metrics.get(id).and_then(|m| m.clock_graphics)
    }

    fn clock_memory(&self, id: usize) -> Option<u32> {
        self.metrics.get(id).and_then(|m| m.clock_memory)
    }
}

#[test]
fn candidates_include_the_nixos_driver_directory() {
    for candidates in [
        dynlib::nvml_candidates(),
        dynlib::amdsmi_candidates(),
        dynlib::level_zero_candidates(),
    ] {
        assert!(
            candidates
                .iter()
                .any(|c| c.starts_with("/run/opengl-driver/lib/")),
            "no NixOS driver path in {candidates:?}"
        );
    }
}

#[test]
fn missing_library_returns_library_not_found() {
    let err = dynlib::open_first(&[
        "/nonexistent/path/libnvidia-ml.so.1",
        "/also/missing/libamd_smi.so",
    ])
    .expect_err("should fail to open missing libraries");

    match err {
        GpuError::LibraryNotFound { candidates } => {
            assert!(candidates.contains("libnvidia-ml"));
            assert!(candidates.contains("libamd_smi"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn missing_symbol_on_real_library() {
    // libc is present on every Linux/CI host; resolving a nonsense symbol must
    // surface MissingSymbol without panicking.
    let lib = dynlib::open_first(&["libc.so.6", "libc.so", "libSystem.dylib", "msvcrt.dll"])
        .expect("failed to open a system C library for the missing-symbol test");

    let err = unsafe {
        dynlib::resolve_required::<unsafe extern "C" fn()>(
            &lib,
            b"plows_gpu_symbol_that_does_not_exist\0",
            "libc",
        )
    }
    .expect_err("expected missing symbol");

    match err {
        GpuError::MissingSymbol { name, library } => {
            assert!(name.contains("plows_gpu_symbol_that_does_not_exist"));
            assert_eq!(library, "libc");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn successful_symbol_resolve_on_libc() {
    let lib = dynlib::open_first(&["libc.so.6", "libc.so", "libSystem.dylib", "msvcrt.dll"])
        .expect("failed to open libc");
    // strlen exists on POSIX and is a convenient typed check.
    let strlen: unsafe extern "C" fn(*const std::ffi::c_char) -> usize = unsafe {
        dynlib::resolve_required(&lib, b"strlen\0", "libc").expect("strlen should resolve")
    };
    let s = std::ffi::CString::new("abc").expect("CString");
    let len = unsafe { strlen(s.as_ptr()) };
    assert_eq!(len, 3);
}

#[test]
fn discover_never_panics_on_cpu_only() {
    // On machines without GPU libs this yields an empty manager; with GPUs it
    // may load backends. Either way it must not panic.
    let mgr = GpuManager::discover();
    let _ = mgr.backend_count();
    let _ = mgr.all_devices();
}

#[test]
fn multiple_backends_aggregate_devices() {
    let mgr = GpuManager::from_backends(vec![
        Box::new(StubBackend::new(Vendor::Nvidia, 2)),
        Box::new(StubBackend::new(Vendor::Amd, 1)),
    ]);

    assert_eq!(mgr.backend_count(), 2);
    assert!(mgr.has_gpu());
    let devices = mgr.all_devices();
    assert_eq!(devices.len(), 3);
    assert_eq!(devices[0].vendor, Vendor::Nvidia);
    assert_eq!(devices[2].vendor, Vendor::Amd);
}

#[test]
fn backend_selection_skips_empty_manager() {
    let mgr = GpuManager::from_backends(vec![]);
    assert_eq!(mgr.backend_count(), 0);
    assert!(!mgr.has_gpu());
    assert!(mgr.all_devices().is_empty());
}

#[test]
fn stub_metrics_and_snapshot() {
    let backend = StubBackend::new(Vendor::Intel, 1);
    assert_eq!(backend.vendor(), Vendor::Intel);
    assert_eq!(backend.utilization(0), Some(10.0));
    assert!(backend.utilization(99).is_none());
    let snap = backend.snapshot(0).expect("snapshot");
    assert_eq!(snap.utilization, Some(10.0));
    assert_eq!(snap.memory_total, Some(8_000_000_000));
}

#[test]
fn refresh_all_invokes_backends() {
    let mut mgr = GpuManager::from_backends(vec![
        Box::new(StubBackend::new(Vendor::Nvidia, 1)),
        Box::new(StubBackend::new(Vendor::Amd, 1)),
        Box::new(StubBackend::new(Vendor::Intel, 1)),
    ]);
    mgr.refresh_all();
    assert_eq!(mgr.all_devices().len(), 3);
}

#[test]
fn candidate_lists_are_non_empty() {
    assert!(!dynlib::nvml_candidates().is_empty());
    assert!(!dynlib::amdsmi_candidates().is_empty());
    assert!(!dynlib::level_zero_candidates().is_empty());
}

/// When a real library *is* present, loading should succeed rather than report
/// MissingSymbol for the core entry points we require. Marked ignore so default
/// CI stays hermetic.
#[test]
#[ignore = "requires libnvidia-ml.so.1 on the host"]
fn live_nvml_load_smoke() {
    let api = plows_gpu::ffi::nvml::NvmlApi::load().expect("NVML load");
    api.init().expect("NVML init");
}

#[test]
#[ignore = "requires libamd_smi.so on the host"]
fn live_amdsmi_load_smoke() {
    let api = plows_gpu::ffi::amdsmi::AmdSmiApi::load().expect("AMD SMI load");
    api.init().expect("AMD SMI init");
}

#[test]
#[ignore = "requires libze_loader.so on the host"]
fn live_level_zero_load_smoke() {
    let api = plows_gpu::ffi::levelzero::LevelZeroApi::load().expect("Level Zero load");
    api.init().expect("Level Zero init");
}

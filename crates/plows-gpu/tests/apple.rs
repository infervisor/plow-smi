//! Optional real-hardware tests; normal CI exercises the backend math without hardware.
use plows_gpu::Vendor;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use plows_gpu::{AppleBackend, GpuBackend};

#[test]
fn vendor_label_is_stable() {
    assert_eq!(Vendor::Apple.as_str(), "apple");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
#[ignore = "requires an Apple Silicon macOS host"]
fn live_apple_snapshot_and_shutdown() {
    use std::time::{Duration, Instant};
    let mut backend = AppleBackend::try_load().unwrap();
    assert_eq!(backend.vendor(), Vendor::Apple);
    assert_eq!(backend.device_count(), 1);
    assert!(backend.devices()[0].pci_bus_id.is_empty());
    assert!(backend.snapshot(1).is_none());
    std::thread::sleep(Duration::from_millis(1200));
    backend.refresh();
    let snapshot = backend.snapshot(0).unwrap();
    assert!(
        snapshot.utilization.is_some(),
        "GPU residency must be available on the validation host"
    );
    assert!(
        snapshot.memory_total.is_none(),
        "unified RAM must not appear as VRAM"
    );
    let soc = snapshot.soc.unwrap();
    assert!(soc.memory_total_bytes.unwrap() > 0);
    assert!(soc.cpu.cores.unwrap() > 0);
    assert!((0.0..=100.0).contains(&soc.cpu.utilization.unwrap()));
    let started = Instant::now();
    for _ in 0..1000 {
        backend.refresh();
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "refresh must not wait for sample windows"
    );
    let started = Instant::now();
    drop(backend);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "shutdown must interrupt sampler waits"
    );
}

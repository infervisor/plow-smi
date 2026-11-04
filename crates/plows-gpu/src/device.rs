//! Shared device identity types.

/// GPU vendor identity.
///
/// Marked `non_exhaustive` so Apple Metal, Vulkan, OpenCL, and other backends
/// can be added without breaking downstream match expressions that use a
/// wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Vendor {
    /// NVIDIA (NVML / `libnvidia-ml`).
    Nvidia,
    /// AMD (`amdgpu` kernel driver via sysfs).
    Amd,
    /// Intel (Level Zero Sysman / `libze_loader`).
    Intel,
    /// Apple Silicon integrated GPU and SoC telemetry (macOS).
    Apple,
}

impl Vendor {
    /// Stable lowercase string used in logs and labels.
    pub fn as_str(self) -> &'static str {
        match self {
            Vendor::Nvidia => "nvidia",
            Vendor::Amd => "amd",
            Vendor::Intel => "intel",
            Vendor::Apple => "apple",
        }
    }
}

impl std::fmt::Display for Vendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Static identity for a single GPU device.
///
/// Dynamic telemetry (utilization, temperature, …) lives on [`crate::GpuBackend`]
/// rather than on this struct so identity can be cached across refreshes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDevice {
    /// Vendor that owns this device.
    pub vendor: Vendor,
    /// Zero-based index within the vendor backend.
    pub index: usize,
    /// Vendor-provided UUID, or a synthetic fallback.
    pub uuid: String,
    /// PCI bus id (e.g. `00000000:01:00.0`), empty if unavailable.
    pub pci_bus_id: String,
    /// Marketing / product name.
    pub model: String,
    /// Architecture string when the vendor exposes one; otherwise empty.
    pub architecture: String,
    /// Total device memory in bytes, when known.
    pub memory_total: Option<u64>,
    /// Board serial number when known.
    pub serial: Option<String>,
}

impl GpuDevice {
    /// Build a device with only the required identity fields filled in.
    pub fn new(vendor: Vendor, index: usize, uuid: String, model: String) -> Self {
        Self {
            vendor,
            index,
            uuid,
            pci_bus_id: String::new(),
            model,
            architecture: String::new(),
            memory_total: None,
            serial: None,
        }
    }
}

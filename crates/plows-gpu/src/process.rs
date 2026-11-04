//! Process occupancy information for GPUs that expose it (NVML, amdgpu).

/// A process consuming GPU resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuProcessInfo {
    /// Host process ID.
    pub pid: u32,
    /// Zero-based device index within the backend that reported the process.
    pub gpu_index: u32,
    /// GPU memory used by this process in bytes (0 if unknown).
    pub gpu_memory_bytes: u64,
    /// `"C"` (compute), `"G"` (graphics), or `"C+G"`.
    pub process_type: String,
}

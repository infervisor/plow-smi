//! Cross-platform GPU metrics via runtime dynamic loading.
//!
//! # Why dynamic loading?
//!
//! Plow SMI (and this crate) must compile and run on machines that do not
//! have CUDA, ROCm, or Intel oneAPI SDKs installed. Linking vendor libraries at
//! build time would:
//!
//! - fail CI and developer laptops without GPU toolkits,
//! - pull in license/distribution constraints,
//! - prevent a single binary from running on CPU-only and multi-vendor hosts.
//!
//! Instead, each backend calls `dlopen` / `LoadLibrary` (via [`libloading`]) on
//! well-known sonames and resolves symbols with `dlsym` exactly once into typed
//! function-pointer tables. Missing libraries or symbols become
//! [`GpuError`] values; [`GpuManager::discover`] logs and continues.
//!
//! AMD needs no library at all: the open `amdgpu` kernel driver publishes its
//! telemetry and controls through sysfs, so [`AmdBackend`] reads the driver
//! directly (see [`amdgpu`]).
//!
//! # How loading works
//!
//! 1. [`ffi::dynlib::open_first`] tries a candidate list of sonames/paths.
//! 2. Required symbols are resolved into an `*Api` struct; optional symbols are
//!    soft-failed.
//! 3. The `Library` handle is stored inside the API struct so pointers stay
//!    valid for the backend lifetime.
//! 4. Safe backend wrappers call the function pointers and cache metrics.
//!
//! # Adding a vendor
//!
//! 1. Add `src/ffi/<vendor>.rs` with ABI types + `load()` / typed helpers.
//! 2. Add `src/backend/<vendor>.rs` implementing [`GpuBackend`].
//! 3. Probe it from [`GpuManager::discover`] (keep independent try/catch).
//! 4. Extend [`Vendor`] (non_exhaustive) and document candidate sonames.
//!
//! Future backends (Apple Metal, Vulkan, OpenCL, XPUM) follow the same shape
//! without changing the public trait surface.
//!
//! # Example
//!
//! ```no_run
//! use plows_gpu::{GpuBackend, GpuManager};
//!
//! let mut mgr = GpuManager::discover();
//! mgr.refresh_all();
//! for backend in mgr.backends() {
//!     println!("{}: {} device(s)", backend.vendor(), backend.device_count());
//!     for (id, dev) in backend.devices().into_iter().enumerate() {
//!         if let Some(util) = backend.utilization(id) {
//!             println!("  [{}] {} util={util:.1}%", dev.index, dev.model);
//!         }
//!     }
//! }
//! ```

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod amdgpu;
pub mod backend;
pub mod control;
pub mod device;
pub mod error;
pub mod ffi;
pub mod loader;
pub mod metrics;
pub mod process;

pub use control::PowerLimits;
pub use device::{GpuDevice, Vendor};
pub use error::{GpuError, Result};
pub use loader::GpuManager;
pub use metrics::{
    CpuClusterMetrics, DeviceMetrics, EngineMetrics, GpuBackend, SocMetrics, TemperatureSensor,
};
pub use process::GpuProcessInfo;

pub use backend::{AmdBackend, AppleBackend, IntelBackend, NvidiaBackend};

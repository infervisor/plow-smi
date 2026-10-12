//! Shared `libloading` helpers for opening libraries and resolving symbols once.

use libloading::{Library, Symbol};

use crate::error::{GpuError, Result};

/// Open the first candidate path/soname that succeeds.
pub fn open_first(candidates: &[&str]) -> Result<Library> {
    let mut last_err = String::new();
    for path in candidates {
        match unsafe { Library::new(path) } {
            Ok(lib) => return Ok(lib),
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }
    tracing::debug!(error = %last_err, "no candidate library could be loaded");
    Err(GpuError::LibraryNotFound {
        candidates: candidates.join(", "),
    })
}

/// Resolve a required symbol and copy the function pointer out of the `Symbol`
/// guard. The library must outlive any use of the returned pointer.
///
/// # Safety
///
/// `T` must match the actual C ABI of `name`.
pub unsafe fn resolve_required<T: Copy>(lib: &Library, name: &[u8], library: &str) -> Result<T> {
    let sym: Symbol<T> = unsafe { lib.get(name) }.map_err(|_| {
        let display = std::str::from_utf8(name)
            .unwrap_or("?")
            .trim_end_matches('\0');
        GpuError::MissingSymbol {
            name: display.to_string(),
            library: library.to_string(),
        }
    })?;
    Ok(*sym)
}

/// Resolve an optional symbol. Missing symbols yield `None` without error.
///
/// # Safety
///
/// `T` must match the actual C ABI of `name`.
pub unsafe fn resolve_optional<T: Copy>(lib: &Library, name: &[u8]) -> Option<T> {
    let sym: Symbol<T> = unsafe { lib.get(name) }.ok()?;
    Some(*sym)
}

/// Where NixOS exposes the driver's userspace libraries. It is not on the
/// default `dlopen` search path, so vendor libraries are also tried by
/// absolute path under it.
const NIXOS_DRIVER_DIR: &str = "/run/opengl-driver/lib";

/// Candidate library names for a vendor on the current platform.
pub fn nvml_candidates() -> &'static [&'static str] {
    &[
        "libnvidia-ml.so.1",
        "libnvidia-ml.so",
        "/run/opengl-driver/lib/libnvidia-ml.so.1",
        #[cfg(target_os = "windows")]
        "nvml.dll",
    ]
}

/// AMD SMI sonames / common absolute ROCm install paths.
pub fn amdsmi_candidates() -> &'static [&'static str] {
    &[
        "libamd_smi.so",
        "libamd_smi.so.1",
        "/opt/rocm/lib/libamd_smi.so",
        "/opt/rocm/lib/libamd_smi.so.1",
        "/run/opengl-driver/lib/libamd_smi.so",
        #[cfg(target_os = "windows")]
        "amd_smi_dll.dll",
    ]
}

/// Library directories of the host distribution.
///
/// A binary built by Nix has a dynamic loader that searches only the Nix
/// store, so a vendor library found by absolute path (`/opt/rocm/lib/…`) still
/// fails to load: its own dependencies (`libstdc++.so.6`, `libdrm.so.2`) live
/// here, where that loader never looks.
const HOST_LIB_DIRS: &[&str] = &[
    "/lib/x86_64-linux-gnu",
    "/usr/lib/x86_64-linux-gnu",
    "/lib/aarch64-linux-gnu",
    "/usr/lib/aarch64-linux-gnu",
    "/usr/lib64",
    "/lib64",
    "/usr/lib",
    "/opt/rocm/lib",
    NIXOS_DRIVER_DIR,
];

/// Load each soname, so a vendor library that needs it finds it already
/// loaded. The binary's own search path is tried first (under Nix that gives
/// its own, newer `libstdc++`), then the host's directories. A soname that
/// cannot be found is skipped: the vendor library may not need it.
///
/// The returned handles must outlive the vendor library.
pub fn preload(sonames: &[&str]) -> Vec<Library> {
    let mut loaded = Vec::new();
    for soname in sonames {
        let found = unsafe { Library::new(soname) }.ok().or_else(|| {
            HOST_LIB_DIRS
                .iter()
                .map(|d| std::path::Path::new(d).join(soname))
                .filter(|p| p.exists())
                .find_map(|p| unsafe { Library::new(&p) }.ok())
        });
        match found {
            Some(lib) => loaded.push(lib),
            None => tracing::debug!(soname, "preload: not found"),
        }
    }
    loaded
}

/// Level Zero loader sonames.
pub fn level_zero_candidates() -> &'static [&'static str] {
    &[
        "libze_loader.so.1",
        "libze_loader.so",
        "/run/opengl-driver/lib/libze_loader.so.1",
        #[cfg(target_os = "windows")]
        "ze_loader.dll",
    ]
}

/// Decode a NUL-terminated C buffer into an owned Rust string.
pub fn c_string_from_buf(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

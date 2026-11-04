//! Vendor FFI symbol tables loaded via `libloading`.
//!
//! Unsafe code belongs only in this module tree. Higher layers consume the
//! safe wrappers ([`nvml::NvmlApi`], [`levelzero::LevelZeroApi`]).
//!
//! ABI structs mirror vendor C headers; field-level rustdoc is intentionally
//! omitted to avoid duplicating header commentary.

#![allow(missing_docs)]
// Opaque vendor handles are raw pointers by ABI; wrappers stay safe for callers.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) mod apple;
pub mod dynlib;
pub mod levelzero;
pub mod nvml;

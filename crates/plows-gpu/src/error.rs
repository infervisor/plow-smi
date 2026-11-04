//! Error types for GPU library loading, initialization, and queries.

use thiserror::Error;

/// Errors produced by `plows-gpu`.
///
/// Library and symbol failures are expected on CPU-only hosts and must never
/// panic the process — callers should treat [`GpuError::LibraryNotFound`] and
/// [`GpuError::MissingSymbol`] as soft failures during discovery.
#[derive(Debug, Error, Clone)]
pub enum GpuError {
    /// None of the candidate shared library names could be opened.
    #[error("GPU library not found (tried: {candidates})")]
    LibraryNotFound {
        /// Comma-separated list of paths/sonames that were attempted.
        candidates: String,
    },

    /// A required symbol could not be resolved via `dlsym`.
    #[error("missing symbol `{name}` in {library}")]
    MissingSymbol {
        /// Symbol name that failed to resolve.
        name: String,
        /// Library that was loaded.
        library: String,
    },

    /// The vendor's kernel driver is absent or drives no device. Like
    /// [`GpuError::LibraryNotFound`], expected on hosts without that vendor.
    #[error("no devices: {0}")]
    NoDevices(String),

    /// The vendor API initialized but returned a failure status.
    #[error("GPU initialization failed: {0}")]
    InitializationFailed(String),

    /// A per-device query failed.
    #[error("device query failed: {0}")]
    DeviceQueryFailed(String),

    /// The requested operation is not supported on this backend/device.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Catch-all for unexpected failures.
    #[error("unknown GPU error: {0}")]
    Unknown(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, GpuError>;

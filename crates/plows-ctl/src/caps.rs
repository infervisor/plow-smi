//! What a node can do — the `plows-ctl capabilities` contract.
//!
//! This is read by plower's node agent, so its shape is a contract:
//! `contract_version` is bumped on any change a consumer would notice.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cpu::CpuCaps;
use crate::profile::{Profile, Resolution};

/// Version of the `capabilities` / `apply` JSON contract.
pub const CONTRACT_VERSION: u32 = 2;

/// A GPU's power limits in milliwatts. `None` means "not reported", never 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerLimitsMw {
    pub current: Option<u64>,
    pub default: Option<u64>,
    pub min: Option<u64>,
    pub max: Option<u64>,
}

impl From<plows_gpu::PowerLimits> for PowerLimitsMw {
    fn from(p: plows_gpu::PowerLimits) -> Self {
        Self {
            current: p.current_mw,
            default: p.default_mw,
            min: p.min_mw,
            max: p.max_mw,
        }
    }
}

/// One GPU, with its inventory facts as raw numbers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuCaps {
    /// Index across all vendors, in the order `plows-ctl list` prints them.
    pub index: u32,
    pub vendor: String,
    /// Index within the vendor's own backend.
    pub vendor_index: u32,
    pub name: String,
    pub uuid: String,
    pub pci_bus_id: String,
    pub vram_bytes: Option<u64>,
    pub driver_version: Option<String>,
    pub power_limit_mw: PowerLimitsMw,
    /// The setter and limit queries are available. This read-only capability
    /// check does not prove write permission; apply reports driver errors.
    pub power_limit_settable: bool,
    /// Why a write of the current limit was refused, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_limit_error: Option<String>,
    /// NVIDIA only; `None` where the vendor has no such mode.
    pub persistence_mode: Option<bool>,
    /// Performance levels this vendor's control accepts.
    pub perf_levels: Vec<String>,
    pub perf_level: Option<String>,
}

/// The hardware facts profiles are resolved against.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeCaps {
    pub cpu: Option<CpuCaps>,
    pub gpus: Vec<GpuCaps>,
}

/// `plows-ctl capabilities --format json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub contract_version: u32,
    pub plows_ctl_version: String,
    pub cpu: Option<CpuCaps>,
    pub gpus: Vec<GpuCaps>,
    /// What each profile would set on *this* node, and what it cannot.
    pub profiles: BTreeMap<String, Resolution>,
}

impl Capabilities {
    pub fn from_node(node: NodeCaps) -> Self {
        let profiles = Profile::ALL
            .iter()
            .map(|p| (p.as_str().to_string(), crate::profile::resolve(*p, &node)))
            .collect();
        Self {
            contract_version: CONTRACT_VERSION,
            plows_ctl_version: env!("CARGO_PKG_VERSION").to_string(),
            cpu: node.cpu,
            gpus: node.gpus,
            profiles,
        }
    }
}

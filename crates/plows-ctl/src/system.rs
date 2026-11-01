//! The real machine: cpufreq sysfs plus the plows-gpu vendor backends.

use plows_gpu::{AmdBackend, AppleBackend, GpuBackend, IntelBackend, NvidiaBackend, PowerLimits};

use crate::apply::PowerControl;
use crate::caps::{GpuCaps, NodeCaps};
use crate::cpu::CpuFreq;

/// One GPU's place: which backend, and its index there.
#[derive(Clone, Copy)]
enum Slot {
    Nvidia(u32),
    Amd(u32),
    Intel(u32),
    Apple(u32),
}

pub struct SystemControl {
    cpu: CpuFreq,
    nvidia: Option<NvidiaBackend>,
    amd: Option<AmdBackend>,
    intel: Option<IntelBackend>,
    apple: Option<AppleBackend>,
    /// Cross-vendor index → slot, in `plows-ctl list` order.
    slots: Vec<Slot>,
}

impl SystemControl {
    /// Load every vendor backend that is present. A missing one is not an
    /// error: a CPU-only node simply has no GPUs.
    pub fn load() -> Self {
        let nvidia = NvidiaBackend::try_load().ok();
        let amd = AmdBackend::try_load().ok();
        let intel = IntelBackend::try_load().ok();
        let apple = AppleBackend::try_load().ok();
        let mut slots = Vec::new();
        if let Some(b) = &nvidia {
            slots.extend((0..b.device_count() as u32).map(Slot::Nvidia));
        }
        if let Some(b) = &amd {
            slots.extend((0..b.device_count() as u32).map(Slot::Amd));
        }
        if let Some(b) = &intel {
            slots.extend((0..b.device_count() as u32).map(Slot::Intel));
        }
        if let Some(b) = &apple {
            slots.extend((0..b.device_count() as u32).map(Slot::Apple));
        }
        Self {
            cpu: CpuFreq::system(),
            nvidia,
            amd,
            intel,
            apple,
            slots,
        }
    }

    fn limits(&self, slot: Slot) -> PowerLimits {
        match slot {
            Slot::Nvidia(i) => self.nvidia.as_ref().and_then(|b| b.power_limits(i).ok()),
            Slot::Amd(i) => self.amd.as_ref().and_then(|b| b.power_limits(i).ok()),
            Slot::Intel(_) | Slot::Apple(_) => None,
        }
        .unwrap_or_default()
    }

    fn write_limit(&self, slot: Slot, mw: u64) -> Result<(), String> {
        match slot {
            Slot::Nvidia(i) => {
                let b = self.nvidia.as_ref().ok_or("NVML is not loaded")?;
                let mw = u32::try_from(mw).map_err(|_| "limit too large".to_string())?;
                b.set_power_limit(i, mw).map_err(|e| e.to_string())
            }
            Slot::Amd(i) => {
                let b = self.amd.as_ref().ok_or("AMD SMI is not loaded")?;
                b.set_power_limit(i, mw).map_err(|e| e.to_string())
            }
            Slot::Intel(_) => Err("Intel GPUs are read-only (Level Zero)".into()),
            Slot::Apple(_) => {
                Err("Apple Silicon is read-only; power and clock control are unsupported".into())
            }
        }
    }

    fn gpu(&mut self, index: u32, slot: Slot) -> GpuCaps {
        let limits = self.limits(slot);
        let settable = match slot {
            Slot::Nvidia(_) => self
                .nvidia
                .as_ref()
                .is_some_and(|b| b.supports_power_limit()),
            Slot::Amd(_) => self.amd.as_ref().is_some_and(|b| b.supports_power_limit()),
            _ => false,
        } && limits.current_mw.is_some()
            && limits.max_mw.is_some();
        let (vendor, vi, dev, driver, persistence, perf_levels, perf_level) = match slot {
            Slot::Nvidia(i) => {
                let b = self.nvidia.as_ref().unwrap();
                (
                    "nvidia",
                    i,
                    b.devices().into_iter().nth(i as usize),
                    b.driver_version(),
                    b.persistence_mode(i),
                    vec!["auto", "low", "high"],
                    None,
                )
            }
            Slot::Amd(i) => {
                let b = self.amd.as_ref().unwrap();
                let level = match b.perf_level(i) {
                    Some(0) => Some("auto"),
                    Some(1) => Some("low"),
                    Some(2) => Some("high"),
                    Some(3) => Some("manual"),
                    _ => None,
                };
                (
                    "amd",
                    i,
                    b.devices().into_iter().nth(i as usize),
                    b.driver_version(),
                    None,
                    vec!["auto", "low", "high", "manual"],
                    level,
                )
            }
            Slot::Intel(i) => {
                let b = self.intel.as_ref().unwrap();
                (
                    "intel",
                    i,
                    b.devices().into_iter().nth(i as usize),
                    b.driver_version(),
                    None,
                    vec![],
                    None,
                )
            }
            Slot::Apple(i) => {
                let b = self.apple.as_ref().unwrap();
                (
                    "apple",
                    i,
                    b.devices().into_iter().nth(i as usize),
                    None,
                    None,
                    vec![],
                    None,
                )
            }
        };
        let dev = dev.unwrap_or_else(|| {
            plows_gpu::GpuDevice::new(
                plows_gpu::Vendor::Amd,
                vi as usize,
                String::new(),
                String::new(),
            )
        });
        GpuCaps {
            index,
            vendor: vendor.into(),
            vendor_index: vi,
            name: dev.model,
            uuid: dev.uuid,
            pci_bus_id: dev.pci_bus_id,
            vram_bytes: dev.memory_total,
            driver_version: driver,
            power_limit_mw: limits.into(),
            power_limit_settable: settable,
            power_limit_error: (!settable)
                .then(|| "power limit control unavailable (read-only probe)".into()),
            persistence_mode: persistence,
            perf_levels: perf_levels.into_iter().map(str::to_string).collect(),
            perf_level: perf_level.map(str::to_string),
        }
    }
}

impl PowerControl for SystemControl {
    fn read(&mut self) -> NodeCaps {
        let slots = self.slots.clone();
        let gpus = slots
            .iter()
            .enumerate()
            .map(|(i, s)| self.gpu(i as u32, *s))
            .collect();
        NodeCaps {
            cpu: self.cpu.read(),
            gpus,
        }
    }

    fn set_cpu_governor(&mut self, governor: &str) -> Result<(), String> {
        self.cpu.set_governor(governor).map_err(|e| e.to_string())
    }

    fn set_cpu_max_freq_khz(&mut self, khz: u64) -> Result<(), String> {
        self.cpu.set_max_freq_khz(khz).map_err(|e| e.to_string())
    }

    fn set_cpu_epp(&mut self, epp: &str) -> Result<(), String> {
        self.cpu.set_epp(epp).map_err(|e| e.to_string())
    }

    fn set_gpu_power_limit_mw(&mut self, index: u32, mw: u64) -> Result<(), String> {
        let slot = *self
            .slots
            .get(index as usize)
            .ok_or_else(|| format!("no GPU {index}"))?;
        self.write_limit(slot, mw)
    }
}

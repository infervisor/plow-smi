//! AMD SMI FFI: load `libamd_smi.so` and resolve telemetry symbols once.
//!
//! Keeps library ownership on the calling backend (no process-global mutex).

use std::ffi::c_void;

use libloading::Library;

use crate::error::{GpuError, Result};
use crate::ffi::dynlib::{self, c_string_from_buf};

pub const AMDSMI_STATUS_SUCCESS: i32 = 0;
pub const AMDSMI_INIT_AMD_GPUS: u64 = 1 << 1;
pub const AMDSMI_PROCESSOR_TYPE_AMD_GPU: u32 = 1;
pub const AMDSMI_MEM_TYPE_VRAM: u32 = 0;
pub const AMDSMI_CLK_TYPE_SYS: u32 = 0;
pub const AMDSMI_CLK_TYPE_DF: u32 = 1;
pub const AMDSMI_TEMPERATURE_TYPE_EDGE: u32 = 0;
pub const AMDSMI_TEMPERATURE_TYPE_JUNCTION: u32 = 1;
pub const AMDSMI_TEMPERATURE_TYPE_VRAM: u32 = 2;
pub const AMDSMI_TEMP_CURRENT: u32 = 0;
const AMDSMI_MAX_STRING_LENGTH: usize = 256;
const AMDSMI_MAX_NUM_FREQUENCIES: usize = 33;

pub type AmdsmiStatus = i32;
pub type ProcessorHandle = *mut c_void;
pub type SocketHandle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AmdsmiPowerInfo {
    pub socket_power: u64,
    pub current_socket_power: u32,
    pub average_socket_power: u32,
    pub gfx_voltage: u64,
    pub soc_voltage: u64,
    pub mem_voltage: u64,
    pub power_limit: u32,
    _pad: u32,
    reserved: [u64; 18],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AmdsmiFrequencies {
    pub has_deep_sleep: u8,
    _pad0: [u8; 3],
    pub num_supported: u32,
    pub current: u32,
    _pad1: u32,
    pub frequency: [u64; AMDSMI_MAX_NUM_FREQUENCIES],
}

/// `amdsmi_power_cap_info_t`. Units are microwatts on bare-metal Linux.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AmdsmiPowerCapInfo {
    pub power_cap: u64,
    pub default_power_cap: u64,
    pub dpm_cap: u64,
    pub min_power_cap: u64,
    pub max_power_cap: u64,
    reserved: [u64; 3],
}

/// `amdsmi_driver_info_t`.
#[repr(C)]
pub struct AmdsmiDriverInfo {
    pub driver_version: [u8; AMDSMI_MAX_STRING_LENGTH],
    pub driver_date: [u8; AMDSMI_MAX_STRING_LENGTH],
    pub driver_name: [u8; AMDSMI_MAX_STRING_LENGTH],
}

#[repr(C)]
pub struct AmdsmiBoardInfo {
    pub model_number: [u8; AMDSMI_MAX_STRING_LENGTH],
    pub product_serial: [u8; AMDSMI_MAX_STRING_LENGTH],
    pub fru_id: [u8; AMDSMI_MAX_STRING_LENGTH],
    pub product_name: [u8; AMDSMI_MAX_STRING_LENGTH],
    pub manufacturer_name: [u8; AMDSMI_MAX_STRING_LENGTH],
    reserved: [u64; 64],
}

/// Resolved AMD SMI function pointers.
pub struct AmdSmiApi {
    initialized: std::sync::Mutex<bool>,
    amdsmi_init: unsafe extern "C" fn(u64) -> AmdsmiStatus,
    amdsmi_shut_down: unsafe extern "C" fn() -> AmdsmiStatus,
    amdsmi_get_socket_handles: unsafe extern "C" fn(*mut u32, *mut SocketHandle) -> AmdsmiStatus,
    amdsmi_get_processor_handles:
        unsafe extern "C" fn(SocketHandle, *mut u32, *mut ProcessorHandle) -> AmdsmiStatus,
    amdsmi_get_processor_type: unsafe extern "C" fn(ProcessorHandle, *mut u32) -> AmdsmiStatus,
    amdsmi_get_gpu_busy_percent: unsafe extern "C" fn(ProcessorHandle, *mut u32) -> AmdsmiStatus,
    amdsmi_get_gpu_memory_total:
        unsafe extern "C" fn(ProcessorHandle, u32, *mut u64) -> AmdsmiStatus,
    amdsmi_get_gpu_memory_usage:
        unsafe extern "C" fn(ProcessorHandle, u32, *mut u64) -> AmdsmiStatus,
    amdsmi_get_power_info:
        unsafe extern "C" fn(ProcessorHandle, *mut AmdsmiPowerInfo) -> AmdsmiStatus,
    amdsmi_get_clk_freq:
        unsafe extern "C" fn(ProcessorHandle, u32, *mut AmdsmiFrequencies) -> AmdsmiStatus,
    amdsmi_get_temp_metric:
        unsafe extern "C" fn(ProcessorHandle, u32, u32, *mut i64) -> AmdsmiStatus,
    amdsmi_get_gpu_fan_rpms: unsafe extern "C" fn(ProcessorHandle, u32, *mut i64) -> AmdsmiStatus,
    amdsmi_get_gpu_device_uuid:
        unsafe extern "C" fn(ProcessorHandle, *mut u32, *mut u8) -> AmdsmiStatus,
    amdsmi_get_gpu_board_info:
        unsafe extern "C" fn(ProcessorHandle, *mut AmdsmiBoardInfo) -> AmdsmiStatus,
    /// `amdsmi_get_gpu_device_bdf`: fills an `amdsmi_bdf_t`, a packed u64.
    amdsmi_get_gpu_device_bdf:
        Option<unsafe extern "C" fn(ProcessorHandle, *mut u64) -> AmdsmiStatus>,
    /// `amdsmi_get_gpu_bdf_id`: the kernel's BDF id, a different layout.
    amdsmi_get_gpu_bdf_id: Option<unsafe extern "C" fn(ProcessorHandle, *mut u64) -> AmdsmiStatus>,
    amdsmi_get_gpu_perf_level:
        Option<unsafe extern "C" fn(ProcessorHandle, *mut u32) -> AmdsmiStatus>,
    amdsmi_set_gpu_perf_level: Option<unsafe extern "C" fn(ProcessorHandle, u32) -> AmdsmiStatus>,
    amdsmi_set_power_cap: Option<unsafe extern "C" fn(ProcessorHandle, u32, u64) -> AmdsmiStatus>,
    amdsmi_get_power_cap_info:
        Option<unsafe extern "C" fn(ProcessorHandle, u32, *mut AmdsmiPowerCapInfo) -> AmdsmiStatus>,
    amdsmi_get_gpu_driver_info:
        Option<unsafe extern "C" fn(ProcessorHandle, *mut AmdsmiDriverInfo) -> AmdsmiStatus>,
    _lib: Library,
    /// Declared after `_lib` so they are dropped after it.
    _deps: Vec<Library>,
}

// SAFETY: opaque handles + fn pointers; callers synchronize via &mut refresh.
unsafe impl Send for AmdSmiApi {}
unsafe impl Sync for AmdSmiApi {}

impl AmdSmiApi {
    pub fn supports_power_limit(&self) -> bool {
        self.amdsmi_set_power_cap.is_some()
    }
    pub fn load() -> Result<Self> {
        // libamd_smi links libstdc++ and dlopens libdrm at init. Under a Nix
        // build neither is on the loader's path, so load them first.
        let deps = dynlib::preload(&[
            "libstdc++.so.6",
            "libgcc_s.so.1",
            "libdrm.so.2",
            "libdrm_amdgpu.so.1",
        ]);
        let candidates = dynlib::amdsmi_candidates();
        let lib = dynlib::open_first(candidates)?;
        let library = "libamd_smi";

        // SAFETY: symbol types match the AMD SMI C ABI (amdsmi.h).
        unsafe {
            Ok(Self {
                initialized: std::sync::Mutex::new(false),
                amdsmi_init: dynlib::resolve_required(&lib, b"amdsmi_init\0", library)?,
                amdsmi_shut_down: dynlib::resolve_required(&lib, b"amdsmi_shut_down\0", library)?,
                amdsmi_get_socket_handles: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_socket_handles\0",
                    library,
                )?,
                amdsmi_get_processor_handles: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_processor_handles\0",
                    library,
                )?,
                amdsmi_get_processor_type: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_processor_type\0",
                    library,
                )?,
                amdsmi_get_gpu_busy_percent: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_gpu_busy_percent\0",
                    library,
                )?,
                amdsmi_get_gpu_memory_total: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_gpu_memory_total\0",
                    library,
                )?,
                amdsmi_get_gpu_memory_usage: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_gpu_memory_usage\0",
                    library,
                )?,
                amdsmi_get_power_info: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_power_info\0",
                    library,
                )?,
                amdsmi_get_clk_freq: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_clk_freq\0",
                    library,
                )?,
                amdsmi_get_temp_metric: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_temp_metric\0",
                    library,
                )?,
                amdsmi_get_gpu_fan_rpms: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_gpu_fan_rpms\0",
                    library,
                )?,
                amdsmi_get_gpu_device_uuid: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_gpu_device_uuid\0",
                    library,
                )?,
                amdsmi_get_gpu_board_info: dynlib::resolve_required(
                    &lib,
                    b"amdsmi_get_gpu_board_info\0",
                    library,
                )?,
                amdsmi_get_gpu_device_bdf: dynlib::resolve_optional(
                    &lib,
                    b"amdsmi_get_gpu_device_bdf\0",
                ),
                amdsmi_get_gpu_bdf_id: dynlib::resolve_optional(&lib, b"amdsmi_get_gpu_bdf_id\0"),
                amdsmi_get_gpu_perf_level: dynlib::resolve_optional(
                    &lib,
                    b"amdsmi_get_gpu_perf_level\0",
                ),
                amdsmi_set_gpu_perf_level: dynlib::resolve_optional(
                    &lib,
                    b"amdsmi_set_gpu_perf_level\0",
                ),
                amdsmi_set_power_cap: dynlib::resolve_optional(&lib, b"amdsmi_set_power_cap\0"),
                amdsmi_get_power_cap_info: dynlib::resolve_optional(
                    &lib,
                    b"amdsmi_get_power_cap_info\0",
                ),
                amdsmi_get_gpu_driver_info: dynlib::resolve_optional(
                    &lib,
                    b"amdsmi_get_gpu_driver_info\0",
                ),
                _lib: lib,
                _deps: deps,
            })
        }
    }

    pub fn init(&self) -> Result<()> {
        let mut initialized = self.initialized.lock().unwrap_or_else(|e| e.into_inner());
        if *initialized {
            return Ok(());
        }
        let st = unsafe { (self.amdsmi_init)(AMDSMI_INIT_AMD_GPUS) };
        if st != AMDSMI_STATUS_SUCCESS {
            return Err(GpuError::InitializationFailed(format!(
                "amdsmi_init returned {st}"
            )));
        }
        *initialized = true;
        Ok(())
    }

    pub fn shutdown(&self) {
        let mut initialized = self.initialized.lock().unwrap_or_else(|e| e.into_inner());
        if *initialized {
            let _ = unsafe { (self.amdsmi_shut_down)() };
            *initialized = false;
        }
    }

    pub fn enumerate_gpus(&self) -> Result<Vec<ProcessorHandle>> {
        let mut socket_count: u32 = 0;
        let st =
            unsafe { (self.amdsmi_get_socket_handles)(&mut socket_count, std::ptr::null_mut()) };
        if st != AMDSMI_STATUS_SUCCESS || socket_count == 0 {
            return Err(GpuError::DeviceQueryFailed(format!(
                "amdsmi_get_socket_handles count failed ({st})"
            )));
        }

        let mut sockets = vec![std::ptr::null_mut(); socket_count as usize];
        let st =
            unsafe { (self.amdsmi_get_socket_handles)(&mut socket_count, sockets.as_mut_ptr()) };
        if st != AMDSMI_STATUS_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "amdsmi_get_socket_handles list failed ({st})"
            )));
        }

        let mut handles = Vec::new();
        for &socket in &sockets[..socket_count as usize] {
            let mut device_count: u32 = 0;
            let st = unsafe {
                (self.amdsmi_get_processor_handles)(socket, &mut device_count, std::ptr::null_mut())
            };
            if st != AMDSMI_STATUS_SUCCESS || device_count == 0 {
                continue;
            }

            let mut procs = vec![std::ptr::null_mut(); device_count as usize];
            let st = unsafe {
                (self.amdsmi_get_processor_handles)(socket, &mut device_count, procs.as_mut_ptr())
            };
            if st != AMDSMI_STATUS_SUCCESS {
                continue;
            }

            for &proc in &procs[..device_count as usize] {
                let mut ptype: u32 = 0;
                let st = unsafe { (self.amdsmi_get_processor_type)(proc, &mut ptype) };
                if st == AMDSMI_STATUS_SUCCESS && ptype == AMDSMI_PROCESSOR_TYPE_AMD_GPU {
                    handles.push(proc);
                }
            }
        }

        if handles.is_empty() {
            Err(GpuError::DeviceQueryFailed(
                "no AMD GPU processors found".into(),
            ))
        } else {
            Ok(handles)
        }
    }

    pub fn gpu_busy_percent(&self, h: ProcessorHandle) -> Option<f32> {
        let mut busy: u32 = 0;
        if unsafe { (self.amdsmi_get_gpu_busy_percent)(h, &mut busy) } == AMDSMI_STATUS_SUCCESS {
            Some(busy as f32)
        } else {
            None
        }
    }

    pub fn memory_bytes(&self, h: ProcessorHandle) -> (Option<u64>, Option<u64>) {
        let mut total = 0u64;
        let mut used = 0u64;
        let mut val = 0u64;
        let total_ok =
            unsafe { (self.amdsmi_get_gpu_memory_total)(h, AMDSMI_MEM_TYPE_VRAM, &mut val) }
                == AMDSMI_STATUS_SUCCESS;
        if total_ok {
            total = val;
        }
        let used_ok =
            unsafe { (self.amdsmi_get_gpu_memory_usage)(h, AMDSMI_MEM_TYPE_VRAM, &mut val) }
                == AMDSMI_STATUS_SUCCESS;
        if used_ok {
            used = val;
        }
        (
            if total_ok { Some(total) } else { None },
            if used_ok { Some(used) } else { None },
        )
    }

    /// Power draw in watts (best-effort from current_socket_power).
    pub fn power_watts(&self, h: ProcessorHandle) -> Option<f32> {
        let mut info = unsafe { std::mem::zeroed::<AmdsmiPowerInfo>() };
        if unsafe { (self.amdsmi_get_power_info)(h, &mut info) } == AMDSMI_STATUS_SUCCESS {
            Some(info.current_socket_power as f32)
        } else {
            None
        }
    }

    /// Power limit in watts.
    ///
    /// From the power-cap query, whose unit is fixed (µW). `power_info`'s
    /// `power_limit` is documented in watts but is µW on MI300/MI350 boards,
    /// which reported a 1 GW limit; it is only the fallback.
    pub fn power_limit_watts(&self, h: ProcessorHandle) -> Option<f32> {
        if let Some((cap_uw, ..)) = self.power_cap_info_uw(h) {
            if cap_uw > 0 {
                return Some((cap_uw / 1_000_000) as f32);
            }
        }
        let mut info = unsafe { std::mem::zeroed::<AmdsmiPowerInfo>() };
        if unsafe { (self.amdsmi_get_power_info)(h, &mut info) } == AMDSMI_STATUS_SUCCESS
            && info.power_limit > 0
        {
            Some(info.power_limit as f32)
        } else {
            None
        }
    }

    pub fn clock_mhz(&self, h: ProcessorHandle, clk_type: u32) -> Option<u32> {
        let mut freq = unsafe { std::mem::zeroed::<AmdsmiFrequencies>() };
        if unsafe { (self.amdsmi_get_clk_freq)(h, clk_type, &mut freq) } != AMDSMI_STATUS_SUCCESS {
            return None;
        }
        let level = freq.current as usize;
        if level < freq.num_supported as usize && level < freq.frequency.len() {
            let raw = freq.frequency[level];
            Some(if raw > 10_000 {
                (raw / 1_000_000) as u32
            } else {
                raw as u32
            })
        } else {
            None
        }
    }

    pub fn temperature_c(&self, h: ProcessorHandle) -> Option<f32> {
        for sensor in [
            AMDSMI_TEMPERATURE_TYPE_JUNCTION,
            AMDSMI_TEMPERATURE_TYPE_EDGE,
            AMDSMI_TEMPERATURE_TYPE_VRAM,
        ] {
            let mut temp: i64 = 0;
            if unsafe { (self.amdsmi_get_temp_metric)(h, sensor, AMDSMI_TEMP_CURRENT, &mut temp) }
                == AMDSMI_STATUS_SUCCESS
                && temp > 0
            {
                let c = if temp > 1000 { temp / 1000 } else { temp };
                return Some(c as f32);
            }
        }
        None
    }

    pub fn fan_rpm(&self, h: ProcessorHandle) -> Option<f32> {
        let mut rpm: i64 = 0;
        if unsafe { (self.amdsmi_get_gpu_fan_rpms)(h, 0, &mut rpm) } == AMDSMI_STATUS_SUCCESS
            && rpm >= 0
        {
            Some(rpm as f32)
        } else {
            None
        }
    }

    pub fn uuid(&self, h: ProcessorHandle) -> String {
        let mut buf = [0u8; 64];
        let mut len = buf.len() as u32;
        if unsafe { (self.amdsmi_get_gpu_device_uuid)(h, &mut len, buf.as_mut_ptr()) }
            == AMDSMI_STATUS_SUCCESS
        {
            c_string_from_buf(&buf)
        } else {
            String::new()
        }
    }

    pub fn board_info(&self, h: ProcessorHandle) -> Option<(String, Option<String>)> {
        let mut info = unsafe { std::mem::zeroed::<AmdsmiBoardInfo>() };
        if unsafe { (self.amdsmi_get_gpu_board_info)(h, &mut info) } != AMDSMI_STATUS_SUCCESS {
            return None;
        }
        let name = c_string_from_buf(&info.product_name);
        let serial = {
            let s = c_string_from_buf(&info.product_serial);
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        };
        Some((
            if name.is_empty() {
                "AMD GPU".into()
            } else {
                name
            },
            serial,
        ))
    }

    /// PCI address as `dddd:bb:dd.f`.
    ///
    /// The symbol this used to look up (`amdsmi_get_gpu_device_bdf_id`) does
    /// not exist, so the address was always empty. `amdsmi_bdf_t` packs
    /// function:3, device:5, bus:8, domain:48 from the low bit; the kernel's
    /// BDF id puts the domain at bit 32 instead.
    pub fn bdf_id(&self, h: ProcessorHandle) -> Option<String> {
        let fmt = |domain: u64, bus: u64, device: u64, function: u64| {
            format!("{domain:04x}:{bus:02x}:{device:02x}.{function:x}")
        };
        if let Some(f) = self.amdsmi_get_gpu_device_bdf {
            let mut v = 0u64;
            if unsafe { f(h, &mut v) } == AMDSMI_STATUS_SUCCESS {
                return Some(fmt(v >> 16, (v >> 8) & 0xff, (v >> 3) & 0x1f, v & 0x7));
            }
        }
        let f = self.amdsmi_get_gpu_bdf_id?;
        let mut v = 0u64;
        (unsafe { f(h, &mut v) } == AMDSMI_STATUS_SUCCESS).then(|| {
            fmt(
                (v >> 32) & 0xffff,
                (v >> 8) & 0xff,
                (v >> 3) & 0x1f,
                v & 0x7,
            )
        })
    }

    /// Current performance level enum (`0=auto`, `1=low`, `2=high`, `3=manual`).
    pub fn perf_level(&self, h: ProcessorHandle) -> Option<u32> {
        let f = self.amdsmi_get_gpu_perf_level?;
        let mut level = 0u32;
        if unsafe { f(h, &mut level) } == AMDSMI_STATUS_SUCCESS {
            Some(level)
        } else {
            None
        }
    }

    /// Set performance level.
    pub fn set_perf_level(&self, h: ProcessorHandle, level: u32) -> Result<()> {
        let f = self
            .amdsmi_set_gpu_perf_level
            .ok_or_else(|| GpuError::Unsupported("amdsmi_set_gpu_perf_level unavailable".into()))?;
        let st = unsafe { f(h, level) };
        if st != AMDSMI_STATUS_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "amdsmi_set_gpu_perf_level returned {st}"
            )));
        }
        Ok(())
    }

    /// Power cap `(current, default, min, max)` in microwatts, sensor 0.
    pub fn power_cap_info_uw(&self, h: ProcessorHandle) -> Option<(u64, u64, u64, u64)> {
        let f = self.amdsmi_get_power_cap_info?;
        let mut info = unsafe { std::mem::zeroed::<AmdsmiPowerCapInfo>() };
        if unsafe { f(h, 0, &mut info) } != AMDSMI_STATUS_SUCCESS {
            return None;
        }
        Some((
            info.power_cap,
            info.default_power_cap,
            info.min_power_cap,
            info.max_power_cap,
        ))
    }

    /// Kernel driver version (e.g. amdgpu `6.14.14`).
    pub fn driver_version(&self, h: ProcessorHandle) -> Option<String> {
        let f = self.amdsmi_get_gpu_driver_info?;
        let mut info = unsafe { std::mem::zeroed::<AmdsmiDriverInfo>() };
        if unsafe { f(h, &mut info) } != AMDSMI_STATUS_SUCCESS {
            return None;
        }
        let v = c_string_from_buf(&info.driver_version);
        (!v.is_empty()).then_some(v)
    }

    /// Set power cap. Input is milliwatts; AMD SMI expects microwatts.
    pub fn set_power_limit_mw(&self, h: ProcessorHandle, milliwatts: u64) -> Result<()> {
        let f = self
            .amdsmi_set_power_cap
            .ok_or_else(|| GpuError::Unsupported("amdsmi_set_power_cap unavailable".into()))?;
        let cap_uw = milliwatts.saturating_mul(1000);
        let st = unsafe { f(h, 0, cap_uw) };
        if st != AMDSMI_STATUS_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "amdsmi_set_power_cap returned {st}"
            )));
        }
        Ok(())
    }
}

impl Drop for AmdSmiApi {
    fn drop(&mut self) {
        self.shutdown();
    }
}

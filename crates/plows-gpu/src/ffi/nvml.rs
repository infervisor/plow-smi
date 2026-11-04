//! NVML FFI: load `libnvidia-ml.so.1` and resolve monitoring symbols once.

use std::os::raw::{c_char, c_uint, c_ulonglong};

use libloading::Library;

use crate::error::{GpuError, Result};
use crate::ffi::dynlib::{self, c_string_from_buf};

/// Opaque NVML device handle.
pub type NvmlDevice = *mut std::ffi::c_void;

/// NVML return codes of interest.
const NVML_SUCCESS: u32 = 0;
const NVML_ERROR_INSUFFICIENT_SIZE: u32 = 7;

/// Temperature sensor: GPU die.
const NVML_TEMPERATURE_GPU: u32 = 0;
/// Clock type: graphics.
const NVML_CLOCK_GRAPHICS: u32 = 0;
/// Clock type: memory.
const NVML_CLOCK_MEM: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NvmlMemory {
    pub total: c_ulonglong,
    pub free: c_ulonglong,
    pub used: c_ulonglong,
}

/// `nvmlMemory_v2_t`: `used` excludes the driver's `reserved` memory.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NvmlMemoryV2 {
    pub version: c_uint,
    pub total: c_ulonglong,
    pub reserved: c_ulonglong,
    pub free: c_ulonglong,
    pub used: c_ulonglong,
}

/// `nvmlMemory_v2` = `NVML_STRUCT_VERSION(Memory, 2)`.
const NVML_MEMORY_V2: c_uint = std::mem::size_of::<NvmlMemoryV2>() as c_uint | (2 << 24);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NvmlUtilization {
    pub gpu: c_uint,
    pub memory: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NvmlPciInfo {
    pub bus_id_legacy: [c_char; 16],
    pub domain: c_uint,
    pub bus: c_uint,
    pub device: c_uint,
    pub pci_device_id: c_uint,
    pub pci_sub_system_id: c_uint,
    pub bus_id: [c_char; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NvmlProcessInfo {
    pub pid: c_uint,
    pub used_gpu_memory: c_ulonglong,
    pub gpu_instance_id: c_uint,
    pub compute_instance_id: c_uint,
}

/// Resolved NVML function pointers. The `_lib` field keeps the mapping alive.
pub struct NvmlApi {
    initialized: std::sync::Mutex<bool>,
    nvml_init_v2: unsafe extern "C" fn() -> u32,
    nvml_shutdown: unsafe extern "C" fn() -> u32,
    nvml_device_get_count_v2: unsafe extern "C" fn(*mut c_uint) -> u32,
    nvml_device_get_handle_by_index_v2: unsafe extern "C" fn(c_uint, *mut NvmlDevice) -> u32,
    nvml_device_get_name: unsafe extern "C" fn(NvmlDevice, *mut c_char, c_uint) -> u32,
    nvml_device_get_uuid: unsafe extern "C" fn(NvmlDevice, *mut c_char, c_uint) -> u32,
    nvml_device_get_serial: unsafe extern "C" fn(NvmlDevice, *mut c_char, c_uint) -> u32,
    nvml_device_get_pci_info_v3: unsafe extern "C" fn(NvmlDevice, *mut NvmlPciInfo) -> u32,
    nvml_device_get_memory_info: unsafe extern "C" fn(NvmlDevice, *mut NvmlMemory) -> u32,
    nvml_device_get_memory_info_v2:
        Option<unsafe extern "C" fn(NvmlDevice, *mut NvmlMemoryV2) -> u32>,
    nvml_device_get_utilization_rates:
        unsafe extern "C" fn(NvmlDevice, *mut NvmlUtilization) -> u32,
    nvml_device_get_temperature: unsafe extern "C" fn(NvmlDevice, u32, *mut c_uint) -> u32,
    nvml_device_get_power_usage: unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> u32,
    nvml_device_get_enforced_power_limit:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> u32>,
    nvml_device_get_fan_speed: unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> u32,
    nvml_device_get_clock_info: unsafe extern "C" fn(NvmlDevice, u32, *mut c_uint) -> u32,
    nvml_device_get_architecture: Option<unsafe extern "C" fn(NvmlDevice, *mut u32) -> u32>,
    nvml_system_get_driver_version: Option<unsafe extern "C" fn(*mut c_char, c_uint) -> u32>,
    nvml_system_get_cuda_driver_version: Option<unsafe extern "C" fn(*mut i32) -> u32>,
    nvml_device_get_compute_running_processes:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint, *mut NvmlProcessInfo) -> u32>,
    nvml_device_get_graphics_running_processes:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint, *mut NvmlProcessInfo) -> u32>,
    // Control / query extensions (optional — soft-fail if absent).
    nvml_device_get_max_clock_info:
        Option<unsafe extern "C" fn(NvmlDevice, u32, *mut c_uint) -> u32>,
    nvml_device_get_power_management_limit_constraints:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint, *mut c_uint) -> u32>,
    nvml_device_set_power_management_limit: Option<unsafe extern "C" fn(NvmlDevice, c_uint) -> u32>,
    nvml_device_get_power_management_limit:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> u32>,
    nvml_device_get_power_management_default_limit:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> u32>,
    nvml_device_get_persistence_mode: Option<unsafe extern "C" fn(NvmlDevice, *mut u32) -> u32>,
    nvml_device_get_applications_clock:
        Option<unsafe extern "C" fn(NvmlDevice, u32, *mut c_uint) -> u32>,
    nvml_device_get_supported_memory_clocks:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint, *mut c_uint) -> u32>,
    nvml_device_get_supported_graphics_clocks:
        Option<unsafe extern "C" fn(NvmlDevice, c_uint, *mut c_uint, *mut c_uint) -> u32>,
    nvml_device_set_applications_clocks:
        Option<unsafe extern "C" fn(NvmlDevice, c_uint, c_uint) -> u32>,
    nvml_device_reset_applications_clocks: Option<unsafe extern "C" fn(NvmlDevice) -> u32>,
    nvml_device_get_curr_pcie_link_generation:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> u32>,
    nvml_device_get_curr_pcie_link_width:
        Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> u32>,
    _lib: Library,
}

// SAFETY: NVML is documented as thread-safe after init; we only store fn pointers.
unsafe impl Send for NvmlApi {}
unsafe impl Sync for NvmlApi {}

impl NvmlApi {
    pub fn supports_power_limit(&self) -> bool {
        self.nvml_device_set_power_management_limit.is_some()
    }
    /// `dlopen` NVML and resolve required symbols.
    pub fn load() -> Result<Self> {
        let candidates = dynlib::nvml_candidates();
        let lib = dynlib::open_first(candidates)?;
        let library = "libnvidia-ml";

        // SAFETY: symbol types match the NVML C ABI.
        unsafe {
            Ok(Self {
                initialized: std::sync::Mutex::new(false),
                nvml_init_v2: dynlib::resolve_required(&lib, b"nvmlInit_v2\0", library)?,
                nvml_shutdown: dynlib::resolve_required(&lib, b"nvmlShutdown\0", library)?,
                nvml_device_get_count_v2: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetCount_v2\0",
                    library,
                )?,
                nvml_device_get_handle_by_index_v2: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetHandleByIndex_v2\0",
                    library,
                )?,
                nvml_device_get_name: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetName\0",
                    library,
                )?,
                nvml_device_get_uuid: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetUUID\0",
                    library,
                )?,
                nvml_device_get_serial: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetSerial\0",
                    library,
                )?,
                nvml_device_get_pci_info_v3: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetPciInfo_v3\0",
                    library,
                )?,
                nvml_device_get_memory_info: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetMemoryInfo\0",
                    library,
                )?,
                nvml_device_get_utilization_rates: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetUtilizationRates\0",
                    library,
                )?,
                nvml_device_get_temperature: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetTemperature\0",
                    library,
                )?,
                nvml_device_get_power_usage: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetPowerUsage\0",
                    library,
                )?,
                nvml_device_get_enforced_power_limit: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetEnforcedPowerLimit\0",
                ),
                nvml_device_get_fan_speed: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetFanSpeed\0",
                    library,
                )?,
                nvml_device_get_clock_info: dynlib::resolve_required(
                    &lib,
                    b"nvmlDeviceGetClockInfo\0",
                    library,
                )?,
                nvml_device_get_memory_info_v2: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetMemoryInfo_v2\0",
                ),
                nvml_device_get_architecture: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetArchitecture\0",
                ),
                nvml_system_get_driver_version: dynlib::resolve_optional(
                    &lib,
                    b"nvmlSystemGetDriverVersion\0",
                ),
                nvml_system_get_cuda_driver_version: dynlib::resolve_optional(
                    &lib,
                    b"nvmlSystemGetCudaDriverVersion_v2\0",
                )
                .or_else(|| dynlib::resolve_optional(&lib, b"nvmlSystemGetCudaDriverVersion\0")),
                nvml_device_get_compute_running_processes: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetComputeRunningProcesses_v3\0",
                )
                .or_else(|| {
                    dynlib::resolve_optional(&lib, b"nvmlDeviceGetComputeRunningProcesses_v2\0")
                }),
                nvml_device_get_graphics_running_processes: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetGraphicsRunningProcesses_v3\0",
                )
                .or_else(|| {
                    dynlib::resolve_optional(&lib, b"nvmlDeviceGetGraphicsRunningProcesses_v2\0")
                }),
                nvml_device_get_max_clock_info: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetMaxClockInfo\0",
                ),
                nvml_device_get_power_management_limit_constraints: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetPowerManagementLimitConstraints\0",
                ),
                nvml_device_set_power_management_limit: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceSetPowerManagementLimit\0",
                ),
                nvml_device_get_power_management_limit: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetPowerManagementLimit\0",
                ),
                nvml_device_get_power_management_default_limit: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetPowerManagementDefaultLimit\0",
                ),
                nvml_device_get_persistence_mode: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetPersistenceMode\0",
                ),
                nvml_device_get_applications_clock: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetApplicationsClock\0",
                ),
                nvml_device_get_supported_memory_clocks: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetSupportedMemoryClocks\0",
                ),
                nvml_device_get_supported_graphics_clocks: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetSupportedGraphicsClocks\0",
                ),
                nvml_device_set_applications_clocks: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceSetApplicationsClocks\0",
                ),
                nvml_device_reset_applications_clocks: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceResetApplicationsClocks\0",
                ),
                nvml_device_get_curr_pcie_link_generation: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetCurrPcieLinkGeneration\0",
                ),
                nvml_device_get_curr_pcie_link_width: dynlib::resolve_optional(
                    &lib,
                    b"nvmlDeviceGetCurrPcieLinkWidth\0",
                ),
                _lib: lib,
            })
        }
    }

    pub fn init(&self) -> Result<()> {
        let mut initialized = self.initialized.lock().unwrap_or_else(|e| e.into_inner());
        if *initialized {
            return Ok(());
        }
        let st = unsafe { (self.nvml_init_v2)() };
        if st != NVML_SUCCESS {
            return Err(GpuError::InitializationFailed(format!(
                "nvmlInit_v2 returned {st}"
            )));
        }
        *initialized = true;
        Ok(())
    }

    pub fn shutdown(&self) {
        let mut initialized = self.initialized.lock().unwrap_or_else(|e| e.into_inner());
        if *initialized {
            let _ = unsafe { (self.nvml_shutdown)() };
            *initialized = false;
        }
    }

    pub fn device_count(&self) -> Result<u32> {
        let mut count = 0u32;
        let st = unsafe { (self.nvml_device_get_count_v2)(&mut count) };
        if st != NVML_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "nvmlDeviceGetCount_v2 returned {st}"
            )));
        }
        Ok(count)
    }

    pub fn device_handle(&self, index: u32) -> Result<NvmlDevice> {
        let mut handle: NvmlDevice = std::ptr::null_mut();
        let st = unsafe { (self.nvml_device_get_handle_by_index_v2)(index, &mut handle) };
        if st != NVML_SUCCESS || handle.is_null() {
            return Err(GpuError::DeviceQueryFailed(format!(
                "nvmlDeviceGetHandleByIndex_v2({index}) returned {st}"
            )));
        }
        Ok(handle)
    }

    pub fn device_name(&self, device: NvmlDevice) -> Option<String> {
        let mut buf = [0 as c_char; 96];
        let st =
            unsafe { (self.nvml_device_get_name)(device, buf.as_mut_ptr(), buf.len() as c_uint) };
        if st == NVML_SUCCESS {
            Some(c_string_from_buf(unsafe {
                std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len())
            }))
        } else {
            None
        }
    }

    pub fn device_uuid(&self, device: NvmlDevice) -> Option<String> {
        let mut buf = [0 as c_char; 96];
        let st =
            unsafe { (self.nvml_device_get_uuid)(device, buf.as_mut_ptr(), buf.len() as c_uint) };
        if st == NVML_SUCCESS {
            Some(c_string_from_buf(unsafe {
                std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len())
            }))
        } else {
            None
        }
    }

    pub fn device_serial(&self, device: NvmlDevice) -> Option<String> {
        let mut buf = [0 as c_char; 96];
        let st =
            unsafe { (self.nvml_device_get_serial)(device, buf.as_mut_ptr(), buf.len() as c_uint) };
        if st == NVML_SUCCESS {
            let s = c_string_from_buf(unsafe {
                std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len())
            });
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            None
        }
    }

    pub fn device_pci_bus_id(&self, device: NvmlDevice) -> Option<String> {
        let mut info = NvmlPciInfo::default();
        let st = unsafe { (self.nvml_device_get_pci_info_v3)(device, &mut info) };
        if st == NVML_SUCCESS {
            Some(c_string_from_buf(unsafe {
                std::slice::from_raw_parts(info.bus_id.as_ptr() as *const u8, info.bus_id.len())
            }))
        } else {
            None
        }
    }

    /// Memory in bytes. `used` excludes driver-reserved memory when the
    /// driver has `nvmlDeviceGetMemoryInfo_v2` (R510+), as `nvidia-smi` does;
    /// v1 folds the reservation into `used`.
    pub fn memory_info(&self, device: NvmlDevice) -> Option<NvmlMemory> {
        if let Some(f) = self.nvml_device_get_memory_info_v2 {
            let mut mem = NvmlMemoryV2 {
                version: NVML_MEMORY_V2,
                ..Default::default()
            };
            if unsafe { f(device, &mut mem) } == NVML_SUCCESS {
                return Some(NvmlMemory {
                    total: mem.total,
                    free: mem.free,
                    used: mem.used,
                });
            }
        }
        let mut mem = NvmlMemory::default();
        let st = unsafe { (self.nvml_device_get_memory_info)(device, &mut mem) };
        if st == NVML_SUCCESS {
            Some(mem)
        } else {
            None
        }
    }

    pub fn utilization(&self, device: NvmlDevice) -> Option<NvmlUtilization> {
        let mut util = NvmlUtilization::default();
        let st = unsafe { (self.nvml_device_get_utilization_rates)(device, &mut util) };
        if st == NVML_SUCCESS {
            Some(util)
        } else {
            None
        }
    }

    pub fn temperature_c(&self, device: NvmlDevice) -> Option<u32> {
        let mut temp = 0u32;
        let st =
            unsafe { (self.nvml_device_get_temperature)(device, NVML_TEMPERATURE_GPU, &mut temp) };
        if st == NVML_SUCCESS {
            Some(temp)
        } else {
            None
        }
    }

    /// Power usage in milliwatts.
    pub fn power_usage_mw(&self, device: NvmlDevice) -> Option<u32> {
        let mut mw = 0u32;
        let st = unsafe { (self.nvml_device_get_power_usage)(device, &mut mw) };
        if st == NVML_SUCCESS {
            Some(mw)
        } else {
            None
        }
    }

    /// Enforced power limit in milliwatts.
    pub fn power_limit_mw(&self, device: NvmlDevice) -> Option<u32> {
        let f = self.nvml_device_get_enforced_power_limit?;
        let mut mw = 0u32;
        let st = unsafe { f(device, &mut mw) };
        if st == NVML_SUCCESS {
            Some(mw)
        } else {
            None
        }
    }

    /// Fan speed as a percentage 0–100.
    pub fn fan_speed_percent(&self, device: NvmlDevice) -> Option<u32> {
        let mut pct = 0u32;
        let st = unsafe { (self.nvml_device_get_fan_speed)(device, &mut pct) };
        if st == NVML_SUCCESS {
            Some(pct)
        } else {
            None
        }
    }

    pub fn clock_mhz(&self, device: NvmlDevice, clock_type: u32) -> Option<u32> {
        let mut mhz = 0u32;
        let st = unsafe { (self.nvml_device_get_clock_info)(device, clock_type, &mut mhz) };
        if st == NVML_SUCCESS {
            Some(mhz)
        } else {
            None
        }
    }

    pub fn clock_graphics_mhz(&self, device: NvmlDevice) -> Option<u32> {
        self.clock_mhz(device, NVML_CLOCK_GRAPHICS)
    }

    pub fn clock_memory_mhz(&self, device: NvmlDevice) -> Option<u32> {
        self.clock_mhz(device, NVML_CLOCK_MEM)
    }

    /// Architecture enum string when `nvmlDeviceGetArchitecture` is present.
    pub fn architecture(&self, device: NvmlDevice) -> Option<String> {
        let f = self.nvml_device_get_architecture?;
        let mut arch = 0u32;
        let st = unsafe { f(device, &mut arch) };
        if st != NVML_SUCCESS {
            return None;
        }
        Some(nvml_arch_name(arch).to_string())
    }

    /// NVIDIA driver version string.
    pub fn driver_version(&self) -> Option<String> {
        let f = self.nvml_system_get_driver_version?;
        let mut buf = [0 as c_char; 80];
        let st = unsafe { f(buf.as_mut_ptr(), buf.len() as c_uint) };
        if st == NVML_SUCCESS {
            Some(c_string_from_buf(unsafe {
                std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len())
            }))
        } else {
            None
        }
    }

    /// CUDA driver version as `"major.minor"`.
    pub fn cuda_version(&self) -> Option<String> {
        let f = self.nvml_system_get_cuda_driver_version?;
        let mut ver: i32 = 0;
        let st = unsafe { f(&mut ver) };
        if st == NVML_SUCCESS && ver > 0 {
            Some(format!("{}.{}", ver / 1000, (ver % 1000) / 10))
        } else {
            None
        }
    }

    /// Running compute/graphics processes for a device.
    pub fn running_processes(&self, device: NvmlDevice) -> Vec<(u32, u64, &'static str)> {
        let mut out = Vec::new();
        self.collect_procs(
            device,
            self.nvml_device_get_compute_running_processes,
            "C",
            &mut out,
        );
        self.collect_procs(
            device,
            self.nvml_device_get_graphics_running_processes,
            "G",
            &mut out,
        );
        out
    }

    fn collect_procs(
        &self,
        device: NvmlDevice,
        f: Option<unsafe extern "C" fn(NvmlDevice, *mut c_uint, *mut NvmlProcessInfo) -> u32>,
        kind: &'static str,
        out: &mut Vec<(u32, u64, &'static str)>,
    ) {
        let Some(f) = f else {
            return;
        };
        let Ok(infos) = query_list(|count, items| unsafe { f(device, count, items) }) else {
            return;
        };
        for info in infos {
            if info.pid == 0 {
                continue;
            }
            // NVML uses max-u64 when memory is unavailable.
            let mem = if info.used_gpu_memory == u64::MAX {
                0
            } else {
                info.used_gpu_memory
            };
            if let Some(existing) = out.iter_mut().find(|(pid, _, _)| *pid == info.pid) {
                existing.1 = existing.1.max(mem);
                existing.2 = "C+G";
            } else {
                out.push((info.pid, mem, kind));
            }
        }
    }

    /// Max graphics / memory clocks in MHz.
    pub fn max_clocks_mhz(&self, device: NvmlDevice) -> Option<(u32, u32)> {
        let f = self.nvml_device_get_max_clock_info?;
        let mut gfx = 0u32;
        let mut mem = 0u32;
        let ok_gfx = unsafe { f(device, NVML_CLOCK_GRAPHICS, &mut gfx) } == NVML_SUCCESS;
        let ok_mem = unsafe { f(device, NVML_CLOCK_MEM, &mut mem) } == NVML_SUCCESS;
        if ok_gfx || ok_mem {
            Some((gfx, mem))
        } else {
            None
        }
    }

    /// Power limit constraints in milliwatts `(min, max)`.
    pub fn power_limit_constraints_mw(&self, device: NvmlDevice) -> Option<(u32, u32)> {
        let f = self.nvml_device_get_power_management_limit_constraints?;
        let mut min = 0u32;
        let mut max = 0u32;
        let st = unsafe { f(device, &mut min, &mut max) };
        if st == NVML_SUCCESS {
            Some((min, max))
        } else {
            None
        }
    }

    /// The power management limit currently set, in milliwatts.
    pub fn power_management_limit_mw(&self, device: NvmlDevice) -> Option<u32> {
        let f = self.nvml_device_get_power_management_limit?;
        let mut mw = 0u32;
        (unsafe { f(device, &mut mw) } == NVML_SUCCESS).then_some(mw)
    }

    /// The board's default power management limit, in milliwatts.
    pub fn power_management_default_limit_mw(&self, device: NvmlDevice) -> Option<u32> {
        let f = self.nvml_device_get_power_management_default_limit?;
        let mut mw = 0u32;
        (unsafe { f(device, &mut mw) } == NVML_SUCCESS).then_some(mw)
    }

    /// Whether persistence mode is on. Without it the driver can unload
    /// between uses and forget a power limit.
    pub fn persistence_mode(&self, device: NvmlDevice) -> Option<bool> {
        let f = self.nvml_device_get_persistence_mode?;
        let mut mode = 0u32;
        (unsafe { f(device, &mut mode) } == NVML_SUCCESS).then_some(mode == 1)
    }

    /// Application clocks `(memory, graphics)` in MHz.
    pub fn applications_clocks_mhz(&self, device: NvmlDevice) -> Option<(u32, u32)> {
        let f = self.nvml_device_get_applications_clock?;
        let (mut mem, mut gfx) = (0u32, 0u32);
        let ok_mem = unsafe { f(device, NVML_CLOCK_MEM, &mut mem) } == NVML_SUCCESS;
        let ok_gfx = unsafe { f(device, NVML_CLOCK_GRAPHICS, &mut gfx) } == NVML_SUCCESS;
        (ok_mem && ok_gfx).then_some((mem, gfx))
    }

    /// Set power management limit in milliwatts.
    pub fn set_power_limit_mw(&self, device: NvmlDevice, milliwatts: u32) -> Result<()> {
        let f = self.nvml_device_set_power_management_limit.ok_or_else(|| {
            GpuError::Unsupported("nvmlDeviceSetPowerManagementLimit unavailable".into())
        })?;
        let st = unsafe { f(device, milliwatts) };
        if st != NVML_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "set power limit returned {st}"
            )));
        }
        Ok(())
    }

    /// Supported memory clocks (MHz), typically descending.
    pub fn supported_memory_clocks(&self, device: NvmlDevice) -> Result<Vec<u32>> {
        let f = self
            .nvml_device_get_supported_memory_clocks
            .ok_or_else(|| {
                GpuError::Unsupported("nvmlDeviceGetSupportedMemoryClocks unavailable".into())
            })?;
        query_list(|count, items| unsafe { f(device, count, items) })
    }

    /// Supported graphics clocks (MHz) for a given memory clock.
    pub fn supported_graphics_clocks(&self, device: NvmlDevice, mem_mhz: u32) -> Result<Vec<u32>> {
        let f = self
            .nvml_device_get_supported_graphics_clocks
            .ok_or_else(|| {
                GpuError::Unsupported("nvmlDeviceGetSupportedGraphicsClocks unavailable".into())
            })?;
        query_list(|count, items| unsafe { f(device, mem_mhz, count, items) })
    }

    /// Set application clocks (memory, graphics) in MHz.
    pub fn set_applications_clocks(
        &self,
        device: NvmlDevice,
        mem_mhz: u32,
        graphics_mhz: u32,
    ) -> Result<()> {
        let f = self.nvml_device_set_applications_clocks.ok_or_else(|| {
            GpuError::Unsupported("nvmlDeviceSetApplicationsClocks unavailable".into())
        })?;
        let st = unsafe { f(device, mem_mhz, graphics_mhz) };
        if st != NVML_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "set applications clocks returned {st}"
            )));
        }
        Ok(())
    }

    /// Reset application clocks to default.
    pub fn reset_applications_clocks(&self, device: NvmlDevice) -> Result<()> {
        let f = self.nvml_device_reset_applications_clocks.ok_or_else(|| {
            GpuError::Unsupported("nvmlDeviceResetApplicationsClocks unavailable".into())
        })?;
        let st = unsafe { f(device) };
        if st != NVML_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "reset applications clocks returned {st}"
            )));
        }
        Ok(())
    }

    /// Current PCIe generation and width.
    pub fn pcie_info(&self, device: NvmlDevice) -> (Option<u32>, Option<u32>) {
        let gen = self
            .nvml_device_get_curr_pcie_link_generation
            .and_then(|f| {
                let mut v = 0u32;
                if unsafe { f(device, &mut v) } == NVML_SUCCESS {
                    Some(v)
                } else {
                    None
                }
            });
        let width = self.nvml_device_get_curr_pcie_link_width.and_then(|f| {
            let mut v = 0u32;
            if unsafe { f(device, &mut v) } == NVML_SUCCESS {
                Some(v)
            } else {
                None
            }
        });
        (gen, width)
    }
}

impl Drop for NvmlApi {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn nvml_arch_name(arch: u32) -> &'static str {
    // Values from nvmlDeviceArchitecture_t; unknown codes fall back generically.
    match arch {
        2 => "Kepler",
        3 => "Maxwell",
        4 => "Pascal",
        5 => "Volta",
        6 => "Turing",
        7 => "Ampere",
        8 => "Ada",
        9 => "Hopper",
        10 => "Blackwell",
        _ => "NVIDIA",
    }
}

/// NVML may grow a list between the count and fill calls. Never consume a
/// buffer on an error, and bound retries when processes are rapidly changing.
fn query_list<T: Default + Clone>(
    mut query: impl FnMut(*mut u32, *mut T) -> u32,
) -> Result<Vec<T>> {
    let mut count = 0;
    let status = query(&mut count, std::ptr::null_mut());
    if status != NVML_SUCCESS && status != NVML_ERROR_INSUFFICIENT_SIZE {
        return Err(GpuError::DeviceQueryFailed(format!(
            "NVML list query returned {status}"
        )));
    }
    for _ in 0..4 {
        if count == 0 {
            return Ok(Vec::new());
        }
        if count > 1_048_576 {
            return Err(GpuError::DeviceQueryFailed(
                "NVML list exceeds safety bound".into(),
            ));
        }
        let mut values = vec![T::default(); count as usize];
        let status = query(&mut count, values.as_mut_ptr());
        if status == NVML_SUCCESS && count as usize <= values.len() {
            values.truncate(count as usize);
            return Ok(values);
        }
        if status != NVML_ERROR_INSUFFICIENT_SIZE {
            return Err(GpuError::DeviceQueryFailed(format!(
                "NVML list query returned {status}"
            )));
        }
    }
    Err(GpuError::DeviceQueryFailed("NVML list kept growing".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn list_retries_growth_and_rejects_invalid_argument() {
        let mut calls = 0;
        let values = query_list::<u32>(|count, out| unsafe {
            calls += 1;
            match calls {
                1 => {
                    *count = 1;
                    NVML_ERROR_INSUFFICIENT_SIZE
                }
                2 => {
                    *count = 2;
                    NVML_ERROR_INSUFFICIENT_SIZE
                }
                _ => {
                    *count = 2;
                    *out = 17;
                    *out.add(1) = 42;
                    NVML_SUCCESS
                }
            }
        })
        .unwrap();
        assert_eq!(values, [17, 42]);
        assert!(query_list::<u32>(|_, _| 2).is_err());
    }
    #[test]
    fn process_layout_matches_nvml_v2_v3_abi() {
        assert_eq!(std::mem::size_of::<NvmlProcessInfo>(), 24);
        assert_eq!(std::mem::offset_of!(NvmlProcessInfo, used_gpu_memory), 8);
    }
    #[test]
    fn memory_v2_layout_matches_nvml_h() {
        assert_eq!(std::mem::size_of::<NvmlMemoryV2>(), 40);
        assert_eq!(std::mem::offset_of!(NvmlMemoryV2, used), 32);
        assert_eq!(NVML_MEMORY_V2, 0x0200_0028);
    }
}

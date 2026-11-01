//! Level Zero loader + Sysman FFI for Intel GPU metrics.
//!
//! ABI layouts are taken from oneAPI Level Zero `ze_api.h` / `zes_api.h`
//! (structure type enums and field order verified against upstream headers).

use std::ffi::c_void;
use std::os::raw::c_char;

use libloading::Library;

use crate::error::{GpuError, Result};
use crate::ffi::dynlib::{self, c_string_from_buf};

pub type ZeResult = u32;
pub type ZeDriverHandle = *mut c_void;
pub type ZeDeviceHandle = *mut c_void;
pub type ZesDeviceHandle = *mut c_void;

const ZE_RESULT_SUCCESS: u32 = 0;
const ZE_INIT_FLAG_GPU_ONLY: u32 = 1;
const ZE_DEVICE_TYPE_GPU: u32 = 1;
const ZE_STRUCTURE_TYPE_DEVICE_PROPERTIES: u32 = 0x3;

const ZES_STRUCTURE_TYPE_DEVICE_PROPERTIES: u32 = 0x1;
const ZES_STRUCTURE_TYPE_PCI_PROPERTIES: u32 = 0x2;
const ZES_STRUCTURE_TYPE_FREQ_STATE: u32 = 0x1b;
const ZES_STRUCTURE_TYPE_MEM_STATE: u32 = 0x1e;
const ZES_FAN_SPEED_UNITS_PERCENT: u32 = 1;
const ZES_STRING_PROPERTY_SIZE: usize = 64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ZeDeviceProperties {
    pub stype: u32,
    pub pnext: *mut c_void,
    pub device_type: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub flags: u32,
    pub subdevice_id: u32,
    pub core_clock_rate: u32,
    pub max_mem_alloc_size: u64,
    pub max_hardware_contexts: u32,
    pub max_command_queue_priority: u32,
    pub num_threads_per_eu: u32,
    pub physical_eu_simd_width: u32,
    pub num_eus_per_subslice: u32,
    pub num_subslices_per_slice: u32,
    pub num_slices: u32,
    pub timer_resolution: u64,
    pub timestamp_valid_bits: u32,
    pub kernel_timestamp_valid_bits: u32,
    pub uuid: [u8; 16],
    pub name: [c_char; 256],
}

impl Default for ZeDeviceProperties {
    fn default() -> Self {
        // SAFETY: zeroed POD; stype set below for zeDeviceGetProperties.
        let mut p: Self = unsafe { std::mem::zeroed() };
        p.stype = ZE_STRUCTURE_TYPE_DEVICE_PROPERTIES;
        p
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ZesMemState {
    pub stype: u32,
    pub pnext: *const c_void,
    pub health: u32,
    pub free: u64,
    pub size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ZesFreqState {
    pub stype: u32,
    pub pnext: *const c_void,
    pub current_voltage: f64,
    pub request: f64,
    pub tdp: f64,
    pub efficient: f64,
    pub actual: f64,
    pub throttle_reasons: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ZesPowerEnergyCounter {
    pub energy: u64,
    pub timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ZesEngineStats {
    pub active_time: u64,
    pub timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ZesPciAddress {
    pub domain: u32,
    pub bus: u32,
    pub device: u32,
    pub function: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ZesPciSpeed {
    pub gen: i32,
    pub width: i32,
    pub max_bandwidth: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ZesPciProperties {
    pub stype: u32,
    pub pnext: *mut c_void,
    pub address: ZesPciAddress,
    pub max_speed: ZesPciSpeed,
    pub have_bandwidth_counters: u8,
    pub have_packet_counters: u8,
    pub have_replay_counters: u8,
}

impl Default for ZesPciProperties {
    fn default() -> Self {
        let mut p: Self = unsafe { std::mem::zeroed() };
        p.stype = ZES_STRUCTURE_TYPE_PCI_PROPERTIES;
        p
    }
}

#[repr(C)]
pub struct ZesDeviceProperties {
    pub stype: u32,
    pub pnext: *mut c_void,
    pub core: ZeDeviceProperties,
    pub num_subdevices: u32,
    pub serial_number: [c_char; ZES_STRING_PROPERTY_SIZE],
    pub board_number: [c_char; ZES_STRING_PROPERTY_SIZE],
    pub brand_name: [c_char; ZES_STRING_PROPERTY_SIZE],
    pub model_name: [c_char; ZES_STRING_PROPERTY_SIZE],
    pub vendor_name: [c_char; ZES_STRING_PROPERTY_SIZE],
    pub driver_version: [c_char; ZES_STRING_PROPERTY_SIZE],
}

impl Default for ZesDeviceProperties {
    fn default() -> Self {
        let mut p: Self = unsafe { std::mem::zeroed() };
        p.stype = ZES_STRUCTURE_TYPE_DEVICE_PROPERTIES;
        p.core.stype = ZE_STRUCTURE_TYPE_DEVICE_PROPERTIES;
        p
    }
}

/// Resolved Level Zero + Sysman entry points. Optional Sysman symbols soft-fail.
pub struct LevelZeroApi {
    ze_init: unsafe extern "C" fn(u32) -> ZeResult,
    ze_driver_get: unsafe extern "C" fn(*mut u32, *mut ZeDriverHandle) -> ZeResult,
    ze_device_get: unsafe extern "C" fn(ZeDriverHandle, *mut u32, *mut ZeDeviceHandle) -> ZeResult,
    ze_device_get_properties:
        unsafe extern "C" fn(ZeDeviceHandle, *mut ZeDeviceProperties) -> ZeResult,
    zes_init: Option<unsafe extern "C" fn(u32) -> ZeResult>,
    zes_driver_get: Option<unsafe extern "C" fn(*mut u32, *mut ZeDriverHandle) -> ZeResult>,
    zes_device_get:
        Option<unsafe extern "C" fn(ZeDriverHandle, *mut u32, *mut ZesDeviceHandle) -> ZeResult>,
    zes_device_get_properties:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut ZesDeviceProperties) -> ZeResult>,
    zes_device_pci_get_properties:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut ZesPciProperties) -> ZeResult>,
    zes_device_enum_memory_modules:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut u32, *mut *mut c_void) -> ZeResult>,
    zes_memory_get_state: Option<unsafe extern "C" fn(*mut c_void, *mut ZesMemState) -> ZeResult>,
    zes_device_enum_temperature_sensors:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut u32, *mut *mut c_void) -> ZeResult>,
    zes_temperature_get_state: Option<unsafe extern "C" fn(*mut c_void, *mut f64) -> ZeResult>,
    zes_device_enum_power_domains:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut u32, *mut *mut c_void) -> ZeResult>,
    zes_power_get_energy_counter:
        Option<unsafe extern "C" fn(*mut c_void, *mut ZesPowerEnergyCounter) -> ZeResult>,
    zes_device_enum_freq_domains:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut u32, *mut *mut c_void) -> ZeResult>,
    zes_frequency_get_state:
        Option<unsafe extern "C" fn(*mut c_void, *mut ZesFreqState) -> ZeResult>,
    zes_device_enum_fans:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut u32, *mut *mut c_void) -> ZeResult>,
    zes_fan_get_state: Option<unsafe extern "C" fn(*mut c_void, u32, *mut i32) -> ZeResult>,
    zes_device_enum_engine_groups:
        Option<unsafe extern "C" fn(ZesDeviceHandle, *mut u32, *mut *mut c_void) -> ZeResult>,
    zes_engine_get_activity:
        Option<unsafe extern "C" fn(*mut c_void, *mut ZesEngineStats) -> ZeResult>,
    _lib: Library,
}

// SAFETY: opaque handles + fn pointers only.
unsafe impl Send for LevelZeroApi {}
unsafe impl Sync for LevelZeroApi {}

impl LevelZeroApi {
    /// `dlopen` the Level Zero loader and resolve core + optional Sysman symbols.
    pub fn load() -> Result<Self> {
        let candidates = dynlib::level_zero_candidates();
        let lib = dynlib::open_first(candidates)?;
        let library = "libze_loader";

        // SAFETY: symbol types match oneAPI Level Zero C ABI.
        unsafe {
            Ok(Self {
                ze_init: dynlib::resolve_required(&lib, b"zeInit\0", library)?,
                ze_driver_get: dynlib::resolve_required(&lib, b"zeDriverGet\0", library)?,
                ze_device_get: dynlib::resolve_required(&lib, b"zeDeviceGet\0", library)?,
                ze_device_get_properties: dynlib::resolve_required(
                    &lib,
                    b"zeDeviceGetProperties\0",
                    library,
                )?,
                zes_init: dynlib::resolve_optional(&lib, b"zesInit\0"),
                zes_driver_get: dynlib::resolve_optional(&lib, b"zesDriverGet\0"),
                zes_device_get: dynlib::resolve_optional(&lib, b"zesDeviceGet\0"),
                zes_device_get_properties: dynlib::resolve_optional(
                    &lib,
                    b"zesDeviceGetProperties\0",
                ),
                zes_device_pci_get_properties: dynlib::resolve_optional(
                    &lib,
                    b"zesDevicePciGetProperties\0",
                ),
                zes_device_enum_memory_modules: dynlib::resolve_optional(
                    &lib,
                    b"zesDeviceEnumMemoryModules\0",
                ),
                zes_memory_get_state: dynlib::resolve_optional(&lib, b"zesMemoryGetState\0"),
                zes_device_enum_temperature_sensors: dynlib::resolve_optional(
                    &lib,
                    b"zesDeviceEnumTemperatureSensors\0",
                ),
                zes_temperature_get_state: dynlib::resolve_optional(
                    &lib,
                    b"zesTemperatureGetState\0",
                ),
                zes_device_enum_power_domains: dynlib::resolve_optional(
                    &lib,
                    b"zesDeviceEnumPowerDomains\0",
                ),
                zes_power_get_energy_counter: dynlib::resolve_optional(
                    &lib,
                    b"zesPowerGetEnergyCounter\0",
                ),
                zes_device_enum_freq_domains: dynlib::resolve_optional(
                    &lib,
                    b"zesDeviceEnumFrequencyDomains\0",
                ),
                zes_frequency_get_state: dynlib::resolve_optional(&lib, b"zesFrequencyGetState\0"),
                zes_device_enum_fans: dynlib::resolve_optional(&lib, b"zesDeviceEnumFans\0"),
                zes_fan_get_state: dynlib::resolve_optional(&lib, b"zesFanGetState\0"),
                zes_device_enum_engine_groups: dynlib::resolve_optional(
                    &lib,
                    b"zesDeviceEnumEngineGroups\0",
                ),
                zes_engine_get_activity: dynlib::resolve_optional(&lib, b"zesEngineGetActivity\0"),
                _lib: lib,
            })
        }
    }

    /// Initialize Level Zero (and Sysman when available).
    pub fn init(&self) -> Result<()> {
        let st = unsafe { (self.ze_init)(ZE_INIT_FLAG_GPU_ONLY) };
        if st != ZE_RESULT_SUCCESS {
            let st2 = unsafe { (self.ze_init)(0) };
            if st2 != ZE_RESULT_SUCCESS {
                return Err(GpuError::InitializationFailed(format!(
                    "zeInit returned {st}/{st2}"
                )));
            }
        }
        if let Some(zes_init) = self.zes_init {
            let _ = unsafe { zes_init(0) };
        }
        Ok(())
    }

    /// Enumerate Level Zero GPU devices across all drivers.
    pub fn enumerate_gpu_devices(&self) -> Result<Vec<(ZeDriverHandle, ZeDeviceHandle)>> {
        let mut driver_count = 0u32;
        let st = unsafe { (self.ze_driver_get)(&mut driver_count, std::ptr::null_mut()) };
        if st != ZE_RESULT_SUCCESS || driver_count == 0 {
            return Err(GpuError::DeviceQueryFailed(format!(
                "zeDriverGet count failed ({st})"
            )));
        }

        let mut drivers = vec![std::ptr::null_mut(); driver_count as usize];
        let st = unsafe { (self.ze_driver_get)(&mut driver_count, drivers.as_mut_ptr()) };
        if st != ZE_RESULT_SUCCESS {
            return Err(GpuError::DeviceQueryFailed(format!(
                "zeDriverGet list failed ({st})"
            )));
        }

        let mut out = Vec::new();
        for &driver in drivers.iter().take(driver_count as usize) {
            let mut device_count = 0u32;
            let st =
                unsafe { (self.ze_device_get)(driver, &mut device_count, std::ptr::null_mut()) };
            if st != ZE_RESULT_SUCCESS || device_count == 0 {
                continue;
            }
            let mut devices = vec![std::ptr::null_mut(); device_count as usize];
            let st =
                unsafe { (self.ze_device_get)(driver, &mut device_count, devices.as_mut_ptr()) };
            if st != ZE_RESULT_SUCCESS {
                continue;
            }
            for &device in devices.iter().take(device_count as usize) {
                let mut props = ZeDeviceProperties::default();
                let st = unsafe { (self.ze_device_get_properties)(device, &mut props) };
                if st == ZE_RESULT_SUCCESS && props.device_type == ZE_DEVICE_TYPE_GPU {
                    out.push((driver, device));
                } else if st == ZE_RESULT_SUCCESS {
                    let name = props_name(&props);
                    if !name.is_empty() {
                        out.push((driver, device));
                    }
                }
            }
        }

        if out.is_empty() {
            Err(GpuError::DeviceQueryFailed(
                "no Level Zero GPU devices found".into(),
            ))
        } else {
            Ok(out)
        }
    }

    /// Query core device properties.
    pub fn device_properties(&self, device: ZeDeviceHandle) -> Option<ZeDeviceProperties> {
        let mut props = ZeDeviceProperties::default();
        let st = unsafe { (self.ze_device_get_properties)(device, &mut props) };
        if st == ZE_RESULT_SUCCESS {
            Some(props)
        } else {
            None
        }
    }

    /// Standalone Sysman has its own driver handles and enumeration order.
    /// Match UUIDs instead of passing a core driver handle or a global index.
    pub fn sysman_device(&self, uuid: &[u8; 16]) -> Option<ZesDeviceHandle> {
        let driver_get = self.zes_driver_get?;
        let device_get = self.zes_device_get?;
        let mut count = 0;
        if unsafe { driver_get(&mut count, std::ptr::null_mut()) } != ZE_RESULT_SUCCESS {
            return None;
        }
        let mut drivers = vec![std::ptr::null_mut(); count as usize];
        if unsafe { driver_get(&mut count, drivers.as_mut_ptr()) } != ZE_RESULT_SUCCESS {
            return None;
        }
        drivers.truncate(count as usize);
        for driver in drivers {
            let mut count = 0;
            if unsafe { device_get(driver, &mut count, std::ptr::null_mut()) } != ZE_RESULT_SUCCESS
            {
                continue;
            }
            let mut devices = vec![std::ptr::null_mut(); count as usize];
            if unsafe { device_get(driver, &mut count, devices.as_mut_ptr()) } != ZE_RESULT_SUCCESS
            {
                continue;
            }
            devices.truncate(count as usize);
            for device in devices {
                if self
                    .sysman_device_properties(device)
                    .is_some_and(|p| &p.core.uuid == uuid)
                {
                    return Some(device);
                }
            }
        }
        None
    }

    /// Sysman device identity (serial / model) when available.
    pub fn sysman_device_properties(&self, zes: ZesDeviceHandle) -> Option<ZesDeviceProperties> {
        let f = self.zes_device_get_properties?;
        let mut props = ZesDeviceProperties::default();
        if unsafe { f(zes, &mut props) } == ZE_RESULT_SUCCESS {
            Some(props)
        } else {
            None
        }
    }

    /// PCI BDF string like `0000:03:00.0`.
    pub fn pci_bus_id(&self, zes: ZesDeviceHandle) -> Option<String> {
        let f = self.zes_device_pci_get_properties?;
        let mut props = ZesPciProperties::default();
        if unsafe { f(zes, &mut props) } != ZE_RESULT_SUCCESS {
            return None;
        }
        let a = props.address;
        Some(format!(
            "{:04x}:{:02x}:{:02x}.{:x}",
            a.domain, a.bus, a.device, a.function
        ))
    }

    /// `(used_bytes, total_bytes)` summed across memory modules.
    pub fn memory_state(&self, zes: ZesDeviceHandle) -> Option<(u64, u64)> {
        let enum_mem = self.zes_device_enum_memory_modules?;
        let get_state = self.zes_memory_get_state?;
        let mut count = 0u32;
        let st = unsafe { enum_mem(zes, &mut count, std::ptr::null_mut()) };
        if st != ZE_RESULT_SUCCESS || count == 0 {
            return None;
        }
        let mut modules = vec![std::ptr::null_mut(); count as usize];
        let st = unsafe { enum_mem(zes, &mut count, modules.as_mut_ptr()) };
        if st != ZE_RESULT_SUCCESS {
            return None;
        }
        let mut total = 0u64;
        let mut free = 0u64;
        for &m in modules.iter().take(count as usize) {
            let mut state = ZesMemState {
                stype: ZES_STRUCTURE_TYPE_MEM_STATE,
                ..Default::default()
            };
            if unsafe { get_state(m, &mut state) } == ZE_RESULT_SUCCESS {
                total = total.saturating_add(state.size);
                free = free.saturating_add(state.free);
            }
        }
        if total == 0 {
            None
        } else {
            Some((total.saturating_sub(free), total))
        }
    }

    /// Temperature in Celsius from the first sensor.
    pub fn temperature_c(&self, zes: ZesDeviceHandle) -> Option<f32> {
        let enum_t = self.zes_device_enum_temperature_sensors?;
        let get_t = self.zes_temperature_get_state?;
        let mut count = 0u32;
        if unsafe { enum_t(zes, &mut count, std::ptr::null_mut()) } != ZE_RESULT_SUCCESS
            || count == 0
        {
            return None;
        }
        let mut sensors = vec![std::ptr::null_mut(); count as usize];
        if unsafe { enum_t(zes, &mut count, sensors.as_mut_ptr()) } != ZE_RESULT_SUCCESS {
            return None;
        }
        let mut temp = 0.0f64;
        if unsafe { get_t(sensors[0], &mut temp) } == ZE_RESULT_SUCCESS {
            Some(temp as f32)
        } else {
            None
        }
    }

    /// Actual frequency of the first domain, in MHz.
    pub fn frequency_mhz(&self, zes: ZesDeviceHandle) -> Option<u32> {
        let enum_f = self.zes_device_enum_freq_domains?;
        let get_f = self.zes_frequency_get_state?;
        let mut count = 0u32;
        if unsafe { enum_f(zes, &mut count, std::ptr::null_mut()) } != ZE_RESULT_SUCCESS
            || count == 0
        {
            return None;
        }
        let mut domains = vec![std::ptr::null_mut(); count as usize];
        if unsafe { enum_f(zes, &mut count, domains.as_mut_ptr()) } != ZE_RESULT_SUCCESS {
            return None;
        }
        let mut state = ZesFreqState {
            stype: ZES_STRUCTURE_TYPE_FREQ_STATE,
            ..Default::default()
        };
        if unsafe { get_f(domains[0], &mut state) } == ZE_RESULT_SUCCESS && state.actual >= 0.0 {
            Some(state.actual as u32)
        } else {
            None
        }
    }

    /// Fan speed as a percentage of max.
    pub fn fan_speed_percent(&self, zes: ZesDeviceHandle) -> Option<f32> {
        let enum_f = self.zes_device_enum_fans?;
        let get_f = self.zes_fan_get_state?;
        let mut count = 0u32;
        if unsafe { enum_f(zes, &mut count, std::ptr::null_mut()) } != ZE_RESULT_SUCCESS
            || count == 0
        {
            return None;
        }
        let mut fans = vec![std::ptr::null_mut(); count as usize];
        if unsafe { enum_f(zes, &mut count, fans.as_mut_ptr()) } != ZE_RESULT_SUCCESS {
            return None;
        }
        let mut speed: i32 = 0;
        if unsafe { get_f(fans[0], ZES_FAN_SPEED_UNITS_PERCENT, &mut speed) } == ZE_RESULT_SUCCESS
            && speed >= 0
        {
            Some(speed as f32)
        } else {
            None
        }
    }

    /// Sample energy counter from the first power domain (µJ + µs timestamp).
    pub fn energy_counter(&self, zes: ZesDeviceHandle) -> Option<ZesPowerEnergyCounter> {
        let enum_p = self.zes_device_enum_power_domains?;
        let get_e = self.zes_power_get_energy_counter?;
        let mut count = 0u32;
        if unsafe { enum_p(zes, &mut count, std::ptr::null_mut()) } != ZE_RESULT_SUCCESS
            || count == 0
        {
            return None;
        }
        let mut domains = vec![std::ptr::null_mut(); count as usize];
        if unsafe { enum_p(zes, &mut count, domains.as_mut_ptr()) } != ZE_RESULT_SUCCESS {
            return None;
        }
        let mut counter = ZesPowerEnergyCounter::default();
        if unsafe { get_e(domains[0], &mut counter) } == ZE_RESULT_SUCCESS {
            Some(counter)
        } else {
            None
        }
    }

    /// Sample engine activity from the first engine group.
    pub fn engine_activity(&self, zes: ZesDeviceHandle) -> Option<ZesEngineStats> {
        let enum_e = self.zes_device_enum_engine_groups?;
        let get_a = self.zes_engine_get_activity?;
        let mut count = 0u32;
        if unsafe { enum_e(zes, &mut count, std::ptr::null_mut()) } != ZE_RESULT_SUCCESS
            || count == 0
        {
            return None;
        }
        let mut engines = vec![std::ptr::null_mut(); count as usize];
        if unsafe { enum_e(zes, &mut count, engines.as_mut_ptr()) } != ZE_RESULT_SUCCESS {
            return None;
        }
        let mut stats = ZesEngineStats::default();
        if unsafe { get_a(engines[0], &mut stats) } == ZE_RESULT_SUCCESS {
            Some(stats)
        } else {
            None
        }
    }
}

/// Format a 16-byte UUID as a hyphenated hex string.
pub fn format_uuid(uuid: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5], uuid[6], uuid[7],
        uuid[8], uuid[9], uuid[10], uuid[11], uuid[12], uuid[13], uuid[14], uuid[15]
    )
}

/// Read device name from core properties.
pub fn props_name(props: &ZeDeviceProperties) -> String {
    c_string_from_buf(unsafe {
        std::slice::from_raw_parts(props.name.as_ptr() as *const u8, props.name.len())
    })
}

/// Decode a Sysman C string field, treating `"unknown"` as absent.
pub fn zes_string(buf: &[c_char]) -> Option<String> {
    let s = c_string_from_buf(unsafe {
        std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len())
    });
    if s.is_empty() || s.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod abi_tests {
    use super::*;
    #[test]
    fn pci_properties_uses_8_bit_ze_bool() {
        assert_eq!(std::mem::size_of::<ZesPciProperties>(), 56);
        assert_eq!(
            std::mem::offset_of!(ZesPciProperties, have_replay_counters),
            50
        );
    }
}

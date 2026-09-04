//! Optional NVML sampling behind the `cuda` feature (dlopen, no hard link).

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint, c_void};

use crate::embedder::ResourceUsage;

type NvmlReturn = c_uint;
const NVML_SUCCESS: NvmlReturn = 0;

#[repr(C)]
struct NvmlDevice {
    _private: [u8; 0],
}

#[repr(C)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

type NvmlInit = unsafe extern "C" fn() -> NvmlReturn;
type NvmlShutdown = unsafe extern "C" fn() -> NvmlReturn;
type NvmlDeviceGetHandleByIndex = unsafe extern "C" fn(c_uint, *mut *mut NvmlDevice) -> NvmlReturn;
type NvmlDeviceGetUUID = unsafe extern "C" fn(*mut NvmlDevice, *mut c_char, c_uint) -> NvmlReturn;
type NvmlDeviceGetMemoryInfo = unsafe extern "C" fn(*mut NvmlDevice, *mut NvmlMemory) -> NvmlReturn;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 2;

/// Best-effort NVML sample. Returns unavailable fields when the library or
/// device cannot be queried. Never panics; never fabricates model bytes.
pub fn sample_nvml(device_index: u32) -> ResourceUsage {
    // SAFETY: all NVML/dl* calls are confined here and null-checked.
    unsafe { sample_nvml_inner(device_index) }
}

unsafe fn sample_nvml_inner(device_index: u32) -> ResourceUsage {
    unsafe {
        let path = match std::ffi::CString::new("libnvidia-ml.so.1") {
            Ok(p) => p,
            Err(_) => return ResourceUsage::unavailable(),
        };
        let mut handle = dlopen(path.as_ptr(), RTLD_NOW);
        if handle.is_null() {
            if let Ok(alt) = std::ffi::CString::new("libnvidia-ml.so") {
                handle = dlopen(alt.as_ptr(), RTLD_NOW);
            }
        }
        if handle.is_null() {
            return ResourceUsage::unavailable();
        }

        let init: Option<NvmlInit> =
            sym(handle, b"nvmlInit_v2\0").or_else(|| sym(handle, b"nvmlInit\0"));
        let shutdown: Option<NvmlShutdown> = sym(handle, b"nvmlShutdown\0");
        let get_handle: Option<NvmlDeviceGetHandleByIndex> =
            sym(handle, b"nvmlDeviceGetHandleByIndex_v2\0")
                .or_else(|| sym(handle, b"nvmlDeviceGetHandleByIndex\0"));
        let get_uuid: Option<NvmlDeviceGetUUID> = sym(handle, b"nvmlDeviceGetUUID\0");
        let get_memory: Option<NvmlDeviceGetMemoryInfo> = sym(handle, b"nvmlDeviceGetMemoryInfo\0");

        let (Some(init), Some(shutdown), Some(get_handle), Some(get_uuid), Some(get_memory)) =
            (init, shutdown, get_handle, get_uuid, get_memory)
        else {
            let _ = dlclose(handle);
            return ResourceUsage::unavailable();
        };

        if init() != NVML_SUCCESS {
            let _ = dlclose(handle);
            return ResourceUsage::unavailable();
        }

        let mut device: *mut NvmlDevice = std::ptr::null_mut();
        if get_handle(device_index, &mut device) != NVML_SUCCESS || device.is_null() {
            let _ = shutdown();
            let _ = dlclose(handle);
            return ResourceUsage::unavailable();
        }

        let mut uuid_buf = [0i8; 96];
        let uuid =
            if get_uuid(device, uuid_buf.as_mut_ptr(), uuid_buf.len() as c_uint) == NVML_SUCCESS {
                CStr::from_ptr(uuid_buf.as_ptr())
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            } else {
                None
            };

        let mut mem = NvmlMemory {
            total: 0,
            free: 0,
            used: 0,
        };
        let (used, total) = if get_memory(device, &mut mem) == NVML_SUCCESS {
            (Some(mem.used), Some(mem.total))
        } else {
            (None, None)
        };

        let _ = shutdown();
        let _ = dlclose(handle);

        ResourceUsage {
            device_id: uuid,
            device_used_bytes: used,
            device_total_bytes: total,
            process_used_bytes: None,
            model_used_bytes: None,
        }
    }
}

unsafe fn sym<T>(handle: *mut c_void, name: &[u8]) -> Option<T> {
    unsafe {
        let ptr = dlsym(handle, name.as_ptr() as *const c_char);
        if ptr.is_null() {
            None
        } else {
            Some(std::mem::transmute_copy(&ptr))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvml_never_fabricates_model_bytes() {
        let u = sample_nvml(0);
        assert!(u.model_used_bytes.is_none());
    }
}

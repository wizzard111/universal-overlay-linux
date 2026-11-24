//! Minimal Vulkan layer for Linux: timestamps every vkQueuePresentKHR and
//! writes it to shared memory for the overlay. It does not draw anything,
//! touch any image or change any Vulkan state — one clock read and one
//! store per frame.
//!
//! Loaded by the Vulkan loader as an implicit layer (manifest installed by
//! `universal-overlay --install-layer`); disable per game with
//! `DISABLE_UNIVERSAL_OVERLAY=1`.
#![cfg(target_os = "linux")]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::sync::{Mutex, OnceLock};
use uo_common::shm;

type VkResult = i32;
const VK_SUCCESS: VkResult = 0;
const VK_ERROR_INITIALIZATION_FAILED: VkResult = -3;
const VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO: i32 = 47;
const VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO: i32 = 48;
const VK_LAYER_LINK_INFO: i32 = 0;
const LAYER_NEGOTIATE_INTERFACE_STRUCT: i32 = 1;

type Handle = *mut c_void;
type PfnVoid = Option<unsafe extern "system" fn()>;
type PfnGipa = unsafe extern "system" fn(Handle, *const c_char) -> PfnVoid;
type PfnGdpa = unsafe extern "system" fn(Handle, *const c_char) -> PfnVoid;
type PfnCreateInstance =
    unsafe extern "system" fn(*const c_void, *const c_void, *mut Handle) -> VkResult;
type PfnDestroyInstance = unsafe extern "system" fn(Handle, *const c_void);
type PfnCreateDevice =
    unsafe extern "system" fn(Handle, *const c_void, *const c_void, *mut Handle) -> VkResult;
type PfnDestroyDevice = unsafe extern "system" fn(Handle, *const c_void);
type PfnQueuePresent = unsafe extern "system" fn(Handle, *const PresentInfo) -> VkResult;

#[repr(C)]
struct BaseIn {
    s_type: i32,
    p_next: *const BaseIn,
}

#[repr(C)]
struct LayerInstanceLink {
    p_next: *mut LayerInstanceLink,
    next_gipa: PfnGipa,
    next_gpdpa: *const c_void,
}

#[repr(C)]
struct LayerInstanceCreateInfo {
    s_type: i32,
    p_next: *const c_void,
    function: i32,
    layer_info: *mut LayerInstanceLink,
}

#[repr(C)]
struct LayerDeviceLink {
    p_next: *mut LayerDeviceLink,
    next_gipa: PfnGipa,
    next_gdpa: PfnGdpa,
}

#[repr(C)]
struct LayerDeviceCreateInfo {
    s_type: i32,
    p_next: *const c_void,
    function: i32,
    layer_info: *mut LayerDeviceLink,
}

#[repr(C)]
pub struct NegotiateLayerInterface {
    s_type: i32,
    p_next: *mut c_void,
    loader_layer_interface_version: u32,
    pfn_gipa: Option<PfnGipa>,
    pfn_gdpa: Option<PfnGdpa>,
    pfn_gpdpa: *const c_void,
}

#[repr(C)]
pub struct PresentInfo {
    s_type: i32,
    p_next: *const c_void,
    wait_semaphore_count: u32,
    p_wait_semaphores: *const u64,
    swapchain_count: u32,
    p_swapchains: *const u64,
    p_image_indices: *const u32,
    p_results: *mut i32,
}

struct InstanceData {
    gipa: PfnGipa,
    destroy: Option<PfnDestroyInstance>,
}

struct DeviceData {
    gdpa: PfnGdpa,
    present: Option<PfnQueuePresent>,
    destroy: Option<PfnDestroyDevice>,
}

// Dispatchable handles start with a pointer to the loader's dispatch table;
// that pointer identifies the instance/device (queues share the device's).
unsafe fn key(h: Handle) -> usize {
    *(h as *const usize)
}

fn instances() -> &'static Mutex<HashMap<usize, InstanceData>> {
    static M: OnceLock<Mutex<HashMap<usize, InstanceData>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn devices() -> &'static Mutex<HashMap<usize, DeviceData>> {
    static M: OnceLock<Mutex<HashMap<usize, DeviceData>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

macro_rules! lock {
    ($m:expr) => {
        $m.lock().unwrap_or_else(|e| e.into_inner())
    };
}

unsafe fn cast<T: Copy>(f: PfnVoid) -> Option<T> {
    f.map(|f| std::mem::transmute_copy::<unsafe extern "system" fn(), T>(&f))
}

// ---------------------------------------------------------------- shm writer

use shm::writer::record;

/// Remove our shm file when the game exits.
#[used]
#[link_section = ".fini_array"]
static FINI: extern "C" fn() = shm::writer::cleanup;

// ---------------------------------------------------------------- hooks

unsafe extern "system" fn layer_create_instance(
    ci: *const c_void,
    alloc: *const c_void,
    out: *mut Handle,
) -> VkResult {
    let mut p = (*(ci as *const BaseIn)).p_next;
    let mut link: *mut LayerInstanceCreateInfo = std::ptr::null_mut();
    while !p.is_null() {
        let li = p as *mut LayerInstanceCreateInfo;
        if (*p).s_type == VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO
            && (*li).function == VK_LAYER_LINK_INFO
        {
            link = li;
            break;
        }
        p = (*p).p_next;
    }
    if link.is_null() || (*link).layer_info.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let next_gipa = (*(*link).layer_info).next_gipa;
    (*link).layer_info = (*(*link).layer_info).p_next; // advance for the next layer
    let Some(create) = cast::<PfnCreateInstance>(next_gipa(
        std::ptr::null_mut(),
        c"vkCreateInstance".as_ptr(),
    )) else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    let r = create(ci, alloc, out);
    if r == VK_SUCCESS {
        let inst = *out;
        let destroy = cast::<PfnDestroyInstance>(next_gipa(inst, c"vkDestroyInstance".as_ptr()));
        lock!(instances()).insert(
            key(inst),
            InstanceData {
                gipa: next_gipa,
                destroy,
            },
        );
    }
    r
}

unsafe extern "system" fn layer_destroy_instance(inst: Handle, alloc: *const c_void) {
    let d = lock!(instances()).remove(&key(inst));
    if let Some(f) = d.and_then(|d| d.destroy) {
        f(inst, alloc);
    }
}

unsafe extern "system" fn layer_create_device(
    phys: Handle,
    ci: *const c_void,
    alloc: *const c_void,
    out: *mut Handle,
) -> VkResult {
    let mut p = (*(ci as *const BaseIn)).p_next;
    let mut link: *mut LayerDeviceCreateInfo = std::ptr::null_mut();
    while !p.is_null() {
        let li = p as *mut LayerDeviceCreateInfo;
        if (*p).s_type == VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO
            && (*li).function == VK_LAYER_LINK_INFO
        {
            link = li;
            break;
        }
        p = (*p).p_next;
    }
    if link.is_null() || (*link).layer_info.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let next_gipa = (*(*link).layer_info).next_gipa;
    let next_gdpa = (*(*link).layer_info).next_gdpa;
    (*link).layer_info = (*(*link).layer_info).p_next;
    let Some(create) =
        cast::<PfnCreateDevice>(next_gipa(std::ptr::null_mut(), c"vkCreateDevice".as_ptr()))
    else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    let r = create(phys, ci, alloc, out);
    if r == VK_SUCCESS {
        let dev = *out;
        let present = cast::<PfnQueuePresent>(next_gdpa(dev, c"vkQueuePresentKHR".as_ptr()));
        let destroy = cast::<PfnDestroyDevice>(next_gdpa(dev, c"vkDestroyDevice".as_ptr()));
        lock!(devices()).insert(
            key(dev),
            DeviceData {
                gdpa: next_gdpa,
                present,
                destroy,
            },
        );
    }
    r
}

unsafe extern "system" fn layer_destroy_device(dev: Handle, alloc: *const c_void) {
    let d = lock!(devices()).remove(&key(dev));
    if let Some(f) = d.and_then(|d| d.destroy) {
        f(dev, alloc);
    }
}

unsafe extern "system" fn layer_queue_present(queue: Handle, info: *const PresentInfo) -> VkResult {
    let next = lock!(devices()).get(&key(queue)).and_then(|d| d.present);
    if !info.is_null() {
        let n = (*info).swapchain_count as usize;
        let chains = (*info).p_swapchains;
        if n > 0 && !chains.is_null() {
            for i in 0..n {
                record(*chains.add(i));
            }
        }
    }
    match next {
        Some(f) => f(queue, info),
        None => VK_ERROR_INITIALIZATION_FAILED,
    }
}

unsafe fn ours(name: &[u8]) -> PfnVoid {
    let f: *const () = match name {
        b"vkGetInstanceProcAddr" => vkGetInstanceProcAddr as *const (),
        b"vkGetDeviceProcAddr" => vkGetDeviceProcAddr as *const (),
        b"vkCreateInstance" => layer_create_instance as *const (),
        b"vkDestroyInstance" => layer_destroy_instance as *const (),
        b"vkCreateDevice" => layer_create_device as *const (),
        b"vkDestroyDevice" => layer_destroy_device as *const (),
        b"vkQueuePresentKHR" => layer_queue_present as *const (),
        _ => return None,
    };
    Some(std::mem::transmute::<*const (), unsafe extern "system" fn()>(f))
}

#[no_mangle]
pub unsafe extern "system" fn vkGetInstanceProcAddr(
    instance: Handle,
    name: *const c_char,
) -> PfnVoid {
    if name.is_null() {
        return None;
    }
    let n = CStr::from_ptr(name).to_bytes();
    if let Some(f) = ours(n) {
        return Some(f);
    }
    if instance.is_null() {
        return None;
    }
    let gipa = lock!(instances()).get(&key(instance)).map(|d| d.gipa);
    gipa.and_then(|g| g(instance, name))
}

#[no_mangle]
pub unsafe extern "system" fn vkGetDeviceProcAddr(device: Handle, name: *const c_char) -> PfnVoid {
    if name.is_null() {
        return None;
    }
    let n = CStr::from_ptr(name).to_bytes();
    if matches!(
        n,
        b"vkGetDeviceProcAddr" | b"vkDestroyDevice" | b"vkQueuePresentKHR"
    ) {
        return ours(n);
    }
    if device.is_null() {
        return None;
    }
    let gdpa = lock!(devices()).get(&key(device)).map(|d| d.gdpa);
    gdpa.and_then(|g| g(device, name))
}

#[no_mangle]
pub unsafe extern "system" fn vkNegotiateLoaderLayerInterfaceVersion(
    p: *mut NegotiateLayerInterface,
) -> VkResult {
    if p.is_null() || (*p).s_type != LAYER_NEGOTIATE_INTERFACE_STRUCT {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if (*p).loader_layer_interface_version >= 2 {
        (*p).loader_layer_interface_version = 2;
        (*p).pfn_gipa = Some(vkGetInstanceProcAddr);
        (*p).pfn_gdpa = Some(vkGetDeviceProcAddr);
        (*p).pfn_gpdpa = std::ptr::null();
    }
    VK_SUCCESS
}

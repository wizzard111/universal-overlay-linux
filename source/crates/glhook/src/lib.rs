//! OpenGL frame timing for Linux, loaded with LD_PRELOAD
//! (`universal-overlay-run %command%` or the optional session-wide mode).
//!
//! Interposes only the buffer-swap entry points — glXSwapBuffers,
//! eglSwapBuffers(+WithDamage) — records a timestamp in the shared-memory
//! ring and calls the real function. Games that fetch these functions via
//! glXGetProcAddress / eglGetProcAddress / dlsym(handle, …) (SDL does) get
//! our wrappers too. Nothing else in the process is touched.
#![cfg(target_os = "linux")]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_ulong, c_void, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};
use uo_common::shm::writer::{cleanup, record};

#[used]
#[link_section = ".fini_array"]
static FINI: extern "C" fn() = cleanup;

type DlsymFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;

extern "C" {
    fn dlvsym(handle: *mut c_void, symbol: *const c_char, version: *const c_char) -> *mut c_void;
}

/// The real dlsym (we export our own `dlsym`).
fn real_dlsym() -> Option<DlsymFn> {
    static P: AtomicUsize = AtomicUsize::new(0);
    let mut p = P.load(Ordering::Relaxed);
    if p == 0 {
        unsafe {
            for ver in [c"GLIBC_2.34", c"GLIBC_2.2.5", c"GLIBC_2.17", c"GLIBC_2.0"] {
                let f = dlvsym(libc::RTLD_NEXT, c"dlsym".as_ptr(), ver.as_ptr());
                if !f.is_null() {
                    p = f as usize;
                    break;
                }
            }
        }
        P.store(p, Ordering::Relaxed);
    }
    (p != 0).then(|| unsafe { std::mem::transmute::<usize, DlsymFn>(p) })
}

/// Real implementations, filled lazily from whichever lookup the app used.
static REAL_GLX_SWAP: AtomicUsize = AtomicUsize::new(0);
static REAL_EGL_SWAP: AtomicUsize = AtomicUsize::new(0);
static REAL_EGL_SWAP_DMG: AtomicUsize = AtomicUsize::new(0);
static REAL_GLX_GPA: AtomicUsize = AtomicUsize::new(0);
static REAL_EGL_GPA: AtomicUsize = AtomicUsize::new(0);

unsafe fn resolve(slot: &AtomicUsize, name: &CStr) -> usize {
    let mut p = slot.load(Ordering::Relaxed);
    if p == 0 {
        if let Some(d) = real_dlsym() {
            p = d(libc::RTLD_NEXT, name.as_ptr()) as usize;
            if p == 0 {
                // libGL / libEGL may not be in the global scope yet.
                for lib in [c"libGL.so.1", c"libGLX.so.0", c"libEGL.so.1"] {
                    let h = libc::dlopen(lib.as_ptr(), libc::RTLD_LAZY | libc::RTLD_NOLOAD);
                    if !h.is_null() {
                        p = d(h, name.as_ptr()) as usize;
                        if p != 0 {
                            break;
                        }
                    }
                }
            }
        }
        slot.store(p, Ordering::Relaxed);
    }
    p
}

// ---------------------------------------------------------------- hooks

/// GLXDrawable is an XID = C `unsigned long` (4 bytes in 32-bit games).
#[no_mangle]
pub unsafe extern "C" fn glXSwapBuffers(dpy: *mut c_void, drawable: c_ulong) {
    #[allow(clippy::useless_conversion)] // c_ulong is 32 bits in 32-bit games
    record(u64::from(drawable));
    let p = resolve(&REAL_GLX_SWAP, c"glXSwapBuffers");
    if p != 0 {
        let f: unsafe extern "C" fn(*mut c_void, c_ulong) = std::mem::transmute(p);
        f(dpy, drawable);
    }
}

#[no_mangle]
pub unsafe extern "C" fn eglSwapBuffers(dpy: *mut c_void, surface: *mut c_void) -> u32 {
    record(surface as u64);
    let p = resolve(&REAL_EGL_SWAP, c"eglSwapBuffers");
    if p == 0 {
        return 0;
    }
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32 = std::mem::transmute(p);
    f(dpy, surface)
}

#[no_mangle]
pub unsafe extern "C" fn eglSwapBuffersWithDamageKHR(
    dpy: *mut c_void,
    surface: *mut c_void,
    rects: *const c_int,
    n: c_int,
) -> u32 {
    record(surface as u64);
    let mut p = resolve(&REAL_EGL_SWAP_DMG, c"eglSwapBuffersWithDamageKHR");
    if p == 0 {
        p = resolve(&REAL_EGL_SWAP_DMG, c"eglSwapBuffersWithDamageEXT");
    }
    if p == 0 {
        return 0;
    }
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_int, c_int) -> u32 =
        std::mem::transmute(p);
    f(dpy, surface, rects, n)
}

/// Our wrapper for a swap function name, if it is one we hook.
fn wrapper_for(name: &[u8]) -> Option<*mut c_void> {
    Some(match name {
        b"glXSwapBuffers" => glXSwapBuffers as *mut c_void,
        b"eglSwapBuffers" => eglSwapBuffers as *mut c_void,
        b"eglSwapBuffersWithDamageKHR" | b"eglSwapBuffersWithDamageEXT" => {
            eglSwapBuffersWithDamageKHR as *mut c_void
        }
        _ => return None,
    })
}

#[no_mangle]
pub unsafe extern "C" fn glXGetProcAddressARB(name: *const u8) -> *mut c_void {
    glXGetProcAddress(name)
}

#[no_mangle]
pub unsafe extern "C" fn glXGetProcAddress(name: *const u8) -> *mut c_void {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    let p = resolve(&REAL_GLX_GPA, c"glXGetProcAddressARB");
    if p == 0 {
        return std::ptr::null_mut();
    }
    let real: unsafe extern "C" fn(*const u8) -> *mut c_void = std::mem::transmute(p);
    let r = real(name);
    let n = CStr::from_ptr(name as *const c_char).to_bytes();
    if !r.is_null() && n == b"glXSwapBuffers" {
        REAL_GLX_SWAP
            .compare_exchange(0, r as usize, Ordering::Relaxed, Ordering::Relaxed)
            .ok();
        return glXSwapBuffers as *mut c_void;
    }
    r
}

#[no_mangle]
pub unsafe extern "C" fn eglGetProcAddress(name: *const c_char) -> *mut c_void {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    let p = resolve(&REAL_EGL_GPA, c"eglGetProcAddress");
    if p == 0 {
        return std::ptr::null_mut();
    }
    let real: unsafe extern "C" fn(*const c_char) -> *mut c_void = std::mem::transmute(p);
    let r = real(name);
    let n = CStr::from_ptr(name).to_bytes();
    if !r.is_null() {
        let slot = match n {
            b"eglSwapBuffers" => Some(&REAL_EGL_SWAP),
            b"eglSwapBuffersWithDamageKHR" | b"eglSwapBuffersWithDamageEXT" => {
                Some(&REAL_EGL_SWAP_DMG)
            }
            _ => None,
        };
        if let (Some(slot), Some(w)) = (slot, wrapper_for(n)) {
            slot.compare_exchange(0, r as usize, Ordering::Relaxed, Ordering::Relaxed)
                .ok();
            return w;
        }
    }
    r
}

/// SDL and others load libGL themselves and look the swap functions up
/// with dlsym(handle, …), which bypasses LD_PRELOAD — so we wrap dlsym.
#[no_mangle]
pub unsafe extern "C" fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void {
    let Some(real) = real_dlsym() else {
        return std::ptr::null_mut();
    };
    let r = real(handle, name);
    if r.is_null() || name.is_null() {
        return r;
    }
    let n = CStr::from_ptr(name).to_bytes();
    let slot = match n {
        b"glXSwapBuffers" => &REAL_GLX_SWAP,
        b"eglSwapBuffers" => &REAL_EGL_SWAP,
        b"eglSwapBuffersWithDamageKHR" | b"eglSwapBuffersWithDamageEXT" => &REAL_EGL_SWAP_DMG,
        b"glXGetProcAddress" | b"glXGetProcAddressARB" => {
            REAL_GLX_GPA
                .compare_exchange(0, r as usize, Ordering::Relaxed, Ordering::Relaxed)
                .ok();
            return glXGetProcAddress as *mut c_void;
        }
        b"eglGetProcAddress" => {
            REAL_EGL_GPA
                .compare_exchange(0, r as usize, Ordering::Relaxed, Ordering::Relaxed)
                .ok();
            return eglGetProcAddress as *mut c_void;
        }
        _ => return r,
    };
    // Don't wrap our own symbols (lookup through the preload scope).
    if r == wrapper_for(n).unwrap_or(std::ptr::null_mut()) {
        return r;
    }
    slot.compare_exchange(0, r as usize, Ordering::Relaxed, Ordering::Relaxed)
        .ok();
    wrapper_for(n).unwrap_or(r)
}

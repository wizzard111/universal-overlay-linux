//! Shared-memory protocol between the Linux Vulkan layer (writer, runs
//! inside the game) and the overlay (reader).
//!
//! One file per process: `/dev/shm/universal-overlay.<pid>`, containing a
//! [`Ring`]. The layer appends one [`Entry`] per `vkQueuePresentKHR` and
//! then bumps `write`. The reader remembers how far it has read.

use std::sync::atomic::AtomicU64;

pub const MAGIC: u32 = 0x554F_5631; // "UOV1"
pub const CAPACITY: usize = 8192;

#[repr(C)]
pub struct Entry {
    /// CLOCK_MONOTONIC timestamp of the present, nanoseconds.
    pub ts_ns: AtomicU64,
    /// VkSwapchainKHR handle, to tell several swap chains apart.
    pub chain: AtomicU64,
}

#[repr(C)]
pub struct Ring {
    pub magic: u32,
    pub capacity: u32,
    pub pid: u32,
    pub _reserved: u32,
    /// Total number of entries ever written (index = write % capacity).
    pub write: AtomicU64,
    pub entries: [Entry; CAPACITY],
}

pub const SIZE: usize = std::mem::size_of::<Ring>();
pub const PREFIX: &str = "universal-overlay.";
pub const DIR: &str = "/dev/shm";

pub fn path(pid: u32) -> String {
    format!("{DIR}/{PREFIX}{pid}")
}

/// Name of the Vulkan layer and its manifest file.
pub const LAYER_NAME: &str = "VK_LAYER_UNIVERSAL_OVERLAY_frametime";
pub const LAYER_MANIFEST: &str = "universal_overlay.json";
pub const LAYER_LIBRARY: &str = "libuo_vklayer.so";
/// The same layer built for 32-bit games (most DXVK/Proton titles from the
/// D3D9/D3D11 era are 32-bit). A separate manifest with its own layer name:
/// the Vulkan loader skips a layer whose library has the wrong ELF class.
pub const LAYER_NAME_32: &str = "VK_LAYER_UNIVERSAL_OVERLAY_frametime_32";
pub const LAYER_MANIFEST_32: &str = "universal_overlay_32.json";
pub const LAYER_LIBRARY_32: &str = "libuo_vklayer32.so";
/// Setting this env var to 1 disables the (implicit) layer for a game.
pub const LAYER_DISABLE_ENV: &str = "DISABLE_UNIVERSAL_OVERLAY";

/// Writer side, used inside game processes by the Vulkan layer and the
/// OpenGL hook: one clock read and two atomic stores per frame.
pub mod writer {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::OnceLock;

    struct Writer {
        ring: *mut Ring,
    }
    unsafe impl Send for Writer {}
    unsafe impl Sync for Writer {}

    fn writer() -> Option<&'static Writer> {
        static W: OnceLock<Option<Writer>> = OnceLock::new();
        W.get_or_init(|| unsafe {
            let pid = libc::getpid() as u32;
            let p = std::ffi::CString::new(path(pid)).ok()?;
            let fd = libc::open(
                p.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC,
                0o644,
            );
            if fd < 0 {
                return None;
            }
            if libc::ftruncate(fd, SIZE as libc::off_t) != 0 {
                libc::close(fd);
                return None;
            }
            let m = libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            libc::close(fd);
            if m == libc::MAP_FAILED {
                return None;
            }
            let ring = m as *mut Ring;
            (*ring).capacity = CAPACITY as u32;
            (*ring).pid = pid;
            (*ring).magic = MAGIC;
            Some(Writer { ring })
        })
        .as_ref()
    }

    /// Record one present of swap chain / drawable `chain`.
    pub fn record(chain: u64) {
        let Some(w) = writer() else { return };
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        let ns = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
        unsafe {
            let r = &*w.ring;
            let idx = r.write.load(Ordering::Relaxed);
            let e = &r.entries[(idx % CAPACITY as u64) as usize];
            e.ts_ns.store(ns, Ordering::Relaxed);
            e.chain.store(chain, Ordering::Relaxed);
            r.write.store(idx + 1, Ordering::Release);
        }
    }

    /// Remove this process's ring (call from a library destructor).
    pub extern "C" fn cleanup() {
        unsafe {
            if let Ok(p) = std::ffi::CString::new(path(libc::getpid() as u32)) {
                libc::unlink(p.as_ptr());
            }
        }
    }
}

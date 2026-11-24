//! Linux frame timing: reads the shared-memory rings written by our Vulkan
//! layer (`crates/vklayer`) inside each Vulkan game (native or Proton/DXVK).
//!
//! Source selection: the focused window's process if it presents, else the
//! process that presented most recently. The fallback matters because
//! games inside Steam's container may see a different PID than the host.

use super::{best_stream, Frame, Status, Stream};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use uo_common::shm;

pub const LATENCY_S: f64 = 0.0;

pub fn now() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

pub fn freq() -> f64 {
    1e9
}

struct Mapped {
    ring: *const shm::Ring,
    read: u64,
    streams: HashMap<u64, Stream>,
}

unsafe impl Send for Mapped {}

impl Mapped {
    fn open(pid: u32) -> Option<Self> {
        let path = std::ffi::CString::new(shm::path(pid)).ok()?;
        unsafe {
            let fd = libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if fd < 0 {
                return None;
            }
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(fd, &mut st) != 0 || (st.st_size as usize) < shm::SIZE {
                libc::close(fd);
                return None;
            }
            let p = libc::mmap(
                std::ptr::null_mut(),
                shm::SIZE,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            libc::close(fd);
            if p == libc::MAP_FAILED {
                return None;
            }
            let ring = p as *const shm::Ring;
            if (*ring).magic != shm::MAGIC {
                libc::munmap(p, shm::SIZE);
                return None;
            }
            // Start reading from the last ~2 seconds' worth, not from zero.
            let w = (*ring).write.load(Ordering::Acquire);
            Some(Self {
                ring,
                read: w.saturating_sub(512),
                streams: HashMap::new(),
            })
        }
    }

    fn written(&self) -> u64 {
        unsafe { (*self.ring).write.load(Ordering::Acquire) }
    }

    fn last_ts(&self) -> i64 {
        let w = self.written();
        if w == 0 {
            return 0;
        }
        let e = unsafe { &(*self.ring).entries[((w - 1) % shm::CAPACITY as u64) as usize] };
        e.ts_ns.load(Ordering::Relaxed) as i64
    }

    fn pump(&mut self, keep: i64) {
        let w = self.written();
        let cap = shm::CAPACITY as u64;
        if w.saturating_sub(self.read) > cap {
            self.read = w - cap; // we fell behind; skip what was overwritten
        }
        while self.read < w {
            let e = unsafe { &(*self.ring).entries[(self.read % cap) as usize] };
            let ts = e.ts_ns.load(Ordering::Relaxed) as i64;
            let chain = e.chain.load(Ordering::Relaxed);
            self.read += 1;
            if ts > 0 {
                self.streams
                    .entry(chain)
                    .or_insert_with(|| Stream::new(1))
                    .push(ts, 1e9, keep);
            }
        }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ring as *mut libc::c_void, shm::SIZE) };
    }
}

struct Reader {
    /// X Present listener (OpenGL / any X11 window), if available.
    present: Option<super::xpresent::Listener>,
    target: u32,
    keep: i64,
    maps: HashMap<u32, Mapped>,
    source: Option<u32>,
    last_scan: i64,
}

static READER: Mutex<Option<Reader>> = Mutex::new(None);

fn reader() -> std::sync::MutexGuard<'static, Option<Reader>> {
    READER.lock().unwrap_or_else(|e| e.into_inner())
}

fn alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

impl Reader {
    /// Re-scan /dev/shm (at most once a second) and pick the source process.
    fn rescan(&mut self, now: i64) {
        if now - self.last_scan < 1_000_000_000 && self.source.is_some() {
            return;
        }
        self.last_scan = now;
        let mut pids = Vec::new();
        if let Ok(rd) = std::fs::read_dir(shm::DIR) {
            for e in rd.flatten() {
                let name = e.file_name();
                let Some(pid) = name
                    .to_str()
                    .and_then(|n| n.strip_prefix(shm::PREFIX))
                    .and_then(|p| p.parse::<u32>().ok())
                else {
                    continue;
                };
                // A crashed game leaves its file behind: clean it up.
                // (Only for PIDs visible here; container PIDs are kept.)
                if !alive(pid)
                    && e.metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .is_some_and(|t| t.elapsed().map(|d| d.as_secs() > 60).unwrap_or(false))
                {
                    let _ = std::fs::remove_file(e.path());
                    continue;
                }
                pids.push(pid);
            }
        }
        self.maps.retain(|pid, _| pids.contains(pid));
        for pid in &pids {
            if !self.maps.contains_key(pid) {
                if let Some(m) = Mapped::open(*pid) {
                    self.maps.insert(*pid, m);
                }
            }
        }
        let fresh = |m: &Mapped| now - m.last_ts() < 2_000_000_000;
        self.source = if self.maps.get(&self.target).is_some_and(fresh) {
            Some(self.target)
        } else {
            self.maps
                .iter()
                .filter(|(_, m)| fresh(m))
                .max_by_key(|(_, m)| m.last_ts())
                .map(|(p, _)| *p)
        };
    }
}

pub struct Session;

impl Session {
    pub fn start(history_s: f64) -> Result<Session, Status> {
        let keep = (history_s * 1e9) as i64;
        let present = super::xpresent::Listener::start(keep);
        let have_present = present.is_some();
        *reader() = Some(Reader {
            present,
            target: 0,
            keep: (history_s * 1e9) as i64,
            maps: HashMap::new(),
            source: None,
            last_scan: 0,
        });
        if layer_manifest_installed().is_none() && !have_present {
            return Err(Status::NoLayer);
        }
        Ok(Session)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        *reader() = None;
    }
}

pub fn set_target(pid: u32) {
    if let Some(r) = reader().as_mut() {
        if r.target != pid {
            r.target = pid;
            r.last_scan = 0;
        }
    }
}

pub fn set_history_seconds(s: f64) {
    if let Some(r) = reader().as_mut() {
        r.keep = (s * 1e9) as i64;
        if let Some(p) = &r.present {
            p.shared.lock().unwrap_or_else(|e| e.into_inner()).keep = r.keep;
        }
    }
}

/// Windows to watch with X Present: the active window and its children.
pub fn set_target_windows(windows: Vec<u32>) {
    if let Some(p) = reader().as_ref().and_then(|r| r.present.as_ref()) {
        let mut g = p.shared.lock().unwrap_or_else(|e| e.into_inner());
        if g.want != windows {
            g.want = windows;
            g.dirty = true;
        }
    }
}

pub fn snapshot(tl: super::Tail) -> Option<Vec<Frame>> {
    let mut g = reader();
    let r = g.as_mut()?;
    let t = now();
    r.rescan(t);
    let keep = r.keep;
    let from_layer = |r: &mut Reader| -> Option<Vec<Frame>> {
        let m = r.maps.get_mut(&r.source?)?;
        m.pump(keep);
        best_stream(m.streams.values(), t, 1e9).map(|b| super::tail(&b.frames, 1e9, tl))
    };
    // 1) Vulkan layer data of the focused process (exact, per swap chain).
    if r.source.is_some() && r.source == Some(r.target) {
        if let Some(v) = from_layer(r) {
            return Some(v);
        }
    }
    // 2) X Present events of the focused window (OpenGL and the rest).
    if let Some(p) = &r.present {
        let s = p.shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = best_stream(s.streams.values(), t, 1e9) {
            return Some(super::tail(&b.frames, 1e9, tl));
        }
    }
    // 3) Whatever Vulkan process presented last (e.g. game in a container).
    from_layer(r)
}

// ---------------------------------------------------------------- layer install

fn user_layer_dir() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join("vulkan/implicit_layer.d")
}

/// Path of an installed manifest, if any.
pub fn layer_manifest_installed() -> Option<PathBuf> {
    let candidates = [
        user_layer_dir(),
        PathBuf::from("/etc/vulkan/implicit_layer.d"),
        PathBuf::from("/usr/share/vulkan/implicit_layer.d"),
        PathBuf::from("/usr/local/share/vulkan/implicit_layer.d"),
    ];
    candidates
        .into_iter()
        .map(|d| d.join(shm::LAYER_MANIFEST))
        .find(|p| p.is_file())
}

/// Write the implicit-layer manifests for the current user, pointing at the
/// layer libraries next to the overlay executable (64-bit, and 32-bit if
/// it was shipped).
pub fn install_layer() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir_exe = exe
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let lib = dir_exe.join(shm::LAYER_LIBRARY);
    if !lib.is_file() {
        return Err(format!("not found: {}", lib.display()));
    }
    let dir = user_layer_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let write = |name: &str, lib: &std::path::Path, file: &str| -> Result<PathBuf, String> {
        let manifest = format!(
            r#"{{
  "file_format_version": "1.0.0",
  "layer": {{
    "name": "{name}",
    "type": "GLOBAL",
    "library_path": "{lib}",
    "api_version": "1.3.0",
    "implementation_version": "1",
    "description": "Universal Overlay frame timing (records present timestamps only)",
    "disable_environment": {{ "{env}": "1" }}
  }}
}}
"#,
            lib = lib.display(),
            env = shm::LAYER_DISABLE_ENV
        );
        let path = dir.join(file);
        std::fs::write(&path, manifest).map_err(|e| e.to_string())?;
        Ok(path)
    };
    let path = write(shm::LAYER_NAME, &lib, shm::LAYER_MANIFEST)?;
    let lib32 = dir_exe.join(shm::LAYER_LIBRARY_32);
    if lib32.is_file() {
        write(shm::LAYER_NAME_32, &lib32, shm::LAYER_MANIFEST_32)?;
    } else {
        let _ = std::fs::remove_file(dir.join(shm::LAYER_MANIFEST_32));
    }
    Ok(path)
}

pub fn uninstall_layer() -> Result<(), String> {
    for f in [shm::LAYER_MANIFEST, shm::LAYER_MANIFEST_32] {
        let path = user_layer_dir().join(f);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

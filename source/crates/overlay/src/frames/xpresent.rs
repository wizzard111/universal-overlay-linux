//! Linux, no injection: frame timing for any X11 / XWayland window (OpenGL
//! and Vulkan alike) from the X Present extension. Every DRI3/Present
//! swap of a window produces a PresentCompleteNotify event carrying the
//! server's CLOCK_MONOTONIC timestamp (`ust`, µs). Any client may subscribe
//! to these events for any window, so we just listen.
//!
//! A dedicated xcb connection lives in a small thread that sleeps in poll()
//! and only wakes for events of the watched windows.

use super::Stream;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[repr(C)]
struct ExtReply {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    length: u32,
    present: u8,
    major_opcode: u8,
    first_event: u8,
    first_error: u8,
}

#[repr(C)]
struct VoidCookie {
    sequence: u32,
}

type Conn = *mut c_void;
type FnConnect = unsafe extern "C" fn(*const c_char, *mut c_int) -> Conn;
type FnDisconnect = unsafe extern "C" fn(Conn);
type FnHasError = unsafe extern "C" fn(Conn) -> c_int;
type FnGetExt = unsafe extern "C" fn(Conn, *mut c_void) -> *const ExtReply;
type FnGenId = unsafe extern "C" fn(Conn) -> u32;
type FnSelect = unsafe extern "C" fn(Conn, u32, u32, u32) -> VoidCookie;
type FnFlush = unsafe extern "C" fn(Conn) -> c_int;
type FnFd = unsafe extern "C" fn(Conn) -> c_int;
type FnPoll = unsafe extern "C" fn(Conn) -> *mut u8;

const XCB_GE_GENERIC: u8 = 35;
const PRESENT_COMPLETE_NOTIFY: u16 = 1;
const MASK_COMPLETE_NOTIFY: u32 = 2;

/// Shared between the listener thread and the overlay.
pub(crate) struct Shared {
    /// Windows we want events for (active window + its children).
    pub want: Vec<u32>,
    pub dirty: bool,
    pub streams: HashMap<u32, Stream>,
    pub keep: i64,
}

pub(crate) struct Listener {
    pub shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Xcb {
    _lib: libloading::Library,
    _present: libloading::Library,
    disconnect: FnDisconnect,
    gen_id: FnGenId,
    select: FnSelect,
    flush: FnFlush,
    poll: FnPoll,
    conn: Conn,
    opcode: u8,
    fd: c_int,
}

unsafe impl Send for Xcb {}

impl Xcb {
    fn open() -> Option<Self> {
        unsafe {
            let lib = libloading::Library::new("libxcb.so.1").ok()?;
            let present = libloading::Library::new("libxcb-present.so.0").ok()?;
            let connect: FnConnect = *lib.get(b"xcb_connect\0").ok()?;
            let disconnect: FnDisconnect = *lib.get(b"xcb_disconnect\0").ok()?;
            let has_error: FnHasError = *lib.get(b"xcb_connection_has_error\0").ok()?;
            let get_ext: FnGetExt = *lib.get(b"xcb_get_extension_data\0").ok()?;
            let gen_id: FnGenId = *lib.get(b"xcb_generate_id\0").ok()?;
            let flush: FnFlush = *lib.get(b"xcb_flush\0").ok()?;
            let fd: FnFd = *lib.get(b"xcb_get_file_descriptor\0").ok()?;
            let poll: FnPoll = *lib.get(b"xcb_poll_for_event\0").ok()?;
            let select: FnSelect = *present.get(b"xcb_present_select_input\0").ok()?;
            let ext_id: *mut c_void = *present.get::<*mut c_void>(b"xcb_present_id\0").ok()?;
            let conn = connect(std::ptr::null(), std::ptr::null_mut());
            if conn.is_null() || has_error(conn) != 0 {
                if !conn.is_null() {
                    disconnect(conn);
                }
                return None;
            }
            let ext = get_ext(conn, ext_id);
            if ext.is_null() || (*ext).present == 0 {
                disconnect(conn);
                return None;
            }
            let opcode = (*ext).major_opcode;
            let fd = fd(conn);
            Some(Self {
                _lib: lib,
                _present: present,
                disconnect,
                gen_id,
                select,
                flush,
                poll,
                conn,
                opcode,
                fd,
            })
        }
    }
}

impl Drop for Xcb {
    fn drop(&mut self) {
        unsafe { (self.disconnect)(self.conn) };
    }
}

impl Listener {
    pub fn start(keep: i64) -> Option<Self> {
        let x = Xcb::open()?;
        let shared = Arc::new(Mutex::new(Shared {
            want: Vec::new(),
            dirty: false,
            streams: HashMap::new(),
            keep,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, st) = (shared.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("x-present".into())
            .spawn(move || run(x, sh, st))
            .ok()?;
        Some(Self {
            shared,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(x: Xcb, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    let debug = std::env::var_os("UO_DEBUG").is_some();
    // window -> event id of our subscription
    let mut subs: HashMap<u32, u32> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        // Update subscriptions if the watched window set changed.
        let want = {
            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
            if g.dirty {
                g.dirty = false;
                let g = &mut *g;
                let want = &g.want;
                g.streams.retain(|w, _| want.contains(w));
                Some(g.want.clone())
            } else {
                None
            }
        };
        if let Some(want) = want {
            if debug {
                eprintln!("[x-present] watching windows {want:x?}");
            }
            unsafe {
                subs.retain(|w, eid| {
                    let keep = want.contains(w);
                    if !keep {
                        (x.select)(x.conn, *eid, *w, 0); // unsubscribe
                    }
                    keep
                });
                for w in want {
                    subs.entry(w).or_insert_with(|| {
                        let eid = (x.gen_id)(x.conn);
                        (x.select)(x.conn, eid, w, MASK_COMPLETE_NOTIFY);
                        eid
                    });
                }
                (x.flush)(x.conn);
            }
        }
        // Sleep until events arrive (or 100 ms, to notice target changes).
        let mut pfd = libc::pollfd {
            fd: x.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut pfd, 1, 100) };
        loop {
            let ev = unsafe { (x.poll)(x.conn) };
            if ev.is_null() {
                break;
            }
            unsafe {
                // Generic event layout: [0]=35, [1]=extension opcode,
                // [8..10]=evtype, [10]=kind, [11]=mode, [16..20]=window, [24..32]=ust.
                if *ev == XCB_GE_GENERIC && *ev.add(1) == x.opcode {
                    let evtype = (ev.add(8) as *const u16).read_unaligned();
                    let kind = *ev.add(10);
                    if debug {
                        eprintln!(
                            "[x-present] event type {evtype} kind {kind} mode {}",
                            *ev.add(11)
                        );
                    }
                    if evtype == PRESENT_COMPLETE_NOTIFY && kind == 0 {
                        let window = (ev.add(16) as *const u32).read_unaligned();
                        let ust = (ev.add(24) as *const u64).read_unaligned();
                        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                        let keep = g.keep;
                        g.streams
                            .entry(window)
                            .or_insert_with(|| Stream::new(0))
                            .push(ust as i64 * 1000, 1e9, keep);
                    }
                }
                libc::free(ev as *mut c_void);
            }
        }
    }
    unsafe {
        for (w, eid) in subs {
            (x.select)(x.conn, eid, w, 0);
        }
        (x.flush)(x.conn);
    }
}

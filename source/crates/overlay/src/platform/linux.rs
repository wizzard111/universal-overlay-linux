//! Linux host (X11; also works for XWayland games under a Wayland session).
//!
//! * Overlay: override-redirect ARGB window with an empty input shape
//!   (clicks pass through), drawn with XPutImage.
//! * Hotkey: XGrabKey on the root window. On a pure Wayland desktop bind
//!   `universal-overlay --toggle` to a shortcut in the DE settings instead.
//! * Tray: StatusNotifierItem (KDE, GNOME with AppIndicator extension, …).
//! * IPC: Unix socket; `--toggle`, the tray and the settings app use it.
//!
//! While hidden, the loop sleeps in poll() with no timeout.

use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void, CString};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};
use uo_common::config::MonitorMode;
use uo_common::hotkey::Hotkey;
use uo_common::{ipc, SETTINGS_EXE};
use uo_overlay::core::{icon_pixels, Core, Rect};
use uo_overlay::frames;
use uo_overlay::render::PxRect;
use x11_dl::xlib;

// ---------------------------------------------------------------- X11

type ShapeCombineRectangles = unsafe extern "C" fn(
    *mut xlib::Display,
    xlib::Window,
    c_int,
    c_int,
    c_int,
    *mut xlib::XRectangle,
    c_int,
    c_int,
    c_int,
);
const SHAPE_INPUT: c_int = 2;
const SHAPE_SET: c_int = 0;

unsafe extern "C" fn ignore_x_errors(_: *mut xlib::Display, _: *mut xlib::XErrorEvent) -> c_int {
    0 // e.g. BadAccess when the hotkey is grabbed by another app
}

struct X11 {
    xl: xlib::Xlib,
    xrandr: Option<x11_dl::xrandr::Xrandr>,
    _xext: Option<libloading::Library>,
    dpy: *mut xlib::Display,
    root: xlib::Window,
    win: xlib::Window,
    gc: xlib::GC,
    visual: *mut xlib::Visual,
    depth: c_int,
    geom: (i32, i32, i32, i32),
    mapped: bool,
    atom_active: xlib::Atom,
    atom_pid: xlib::Atom,
    grabbed: Option<(c_int, c_uint)>,
    /// MIT-SHM: the image lives in memory shared with the X server, so
    /// presenting does not push every frame through the X socket.
    xshm: Option<x11_dl::xshm::Xext>,
    shm: Option<ShmImage>,
}

struct ShmImage {
    img: *mut xlib::XImage,
    info: Box<x11_dl::xshm::XShmSegmentInfo>,
    w: i32,
    h: i32,
}

impl X11 {
    fn open() -> Result<Self, String> {
        let xl = xlib::Xlib::open().map_err(|e| format!("libX11: {e}"))?;
        unsafe {
            let dpy = (xl.XOpenDisplay)(std::ptr::null());
            if dpy.is_null() {
                return Err("could not connect to the X server (DISPLAY variable)".into());
            }
            (xl.XSetErrorHandler)(Some(ignore_x_errors));
            let screen = (xl.XDefaultScreen)(dpy);
            let root = (xl.XRootWindow)(dpy, screen);

            // 32-bit ARGB visual for per-pixel transparency (needs a compositor).
            let mut vi: xlib::XVisualInfo = std::mem::zeroed();
            let (visual, depth) =
                if (xl.XMatchVisualInfo)(dpy, screen, 32, xlib::TrueColor, &mut vi) != 0 {
                    (vi.visual, 32)
                } else {
                    (
                        (xl.XDefaultVisual)(dpy, screen),
                        (xl.XDefaultDepth)(dpy, screen),
                    )
                };
            let cmap = (xl.XCreateColormap)(dpy, root, visual, xlib::AllocNone);
            let mut attrs: xlib::XSetWindowAttributes = std::mem::zeroed();
            attrs.colormap = cmap;
            attrs.border_pixel = 0;
            attrs.background_pixel = 0;
            attrs.override_redirect = xlib::True;
            let win = (xl.XCreateWindow)(
                dpy,
                root,
                0,
                0,
                1,
                1,
                0,
                depth,
                xlib::InputOutput as c_uint,
                visual,
                xlib::CWColormap
                    | xlib::CWBorderPixel
                    | xlib::CWBackPixel
                    | xlib::CWOverrideRedirect,
                &mut attrs,
            );
            let title = CString::new("Universal Overlay").unwrap();
            (xl.XStoreName)(dpy, win, title.as_ptr() as *mut c_char);

            // Click-through: empty input shape (XShape lives in libXext).
            let xext = libloading::Library::new("libXext.so.6").ok();
            if let Some(lib) = &xext {
                if let Ok(f) = lib.get::<ShapeCombineRectangles>(b"XShapeCombineRectangles\0") {
                    f(
                        dpy,
                        win,
                        SHAPE_INPUT,
                        0,
                        0,
                        std::ptr::null_mut(),
                        0,
                        SHAPE_SET,
                        0,
                    );
                }
            }
            let gc = (xl.XCreateGC)(dpy, win, 0, std::ptr::null_mut());
            let atom = |n: &str| {
                let c = CString::new(n).unwrap();
                (xl.XInternAtom)(dpy, c.as_ptr(), xlib::False)
            };
            let atom_active = atom("_NET_ACTIVE_WINDOW");
            let atom_pid = atom("_NET_WM_PID");
            let xrandr = x11_dl::xrandr::Xrandr::open().ok();
            let xshm = x11_dl::xshm::Xext::open()
                .ok()
                .filter(|x| (x.XShmQueryExtension)(dpy) != 0);
            Ok(Self {
                xl,
                xrandr,
                _xext: xext,
                dpy,
                root,
                win,
                gc,
                visual,
                depth,
                geom: (0, 0, 1, 1),
                mapped: false,
                atom_active,
                atom_pid,
                grabbed: None,
                xshm,
                shm: None,
            })
        }
    }

    fn fd(&self) -> c_int {
        unsafe { (self.xl.XConnectionNumber)(self.dpy) }
    }

    fn prop_ulong(&self, w: xlib::Window, prop: xlib::Atom, ty: xlib::Atom) -> Option<c_ulong> {
        unsafe {
            let (mut at, mut af, mut n, mut after) = (0, 0, 0, 0);
            let mut data: *mut u8 = std::ptr::null_mut();
            let r = (self.xl.XGetWindowProperty)(
                self.dpy,
                w,
                prop,
                0,
                1,
                xlib::False,
                ty,
                &mut at,
                &mut af,
                &mut n,
                &mut after,
                &mut data,
            );
            if r != 0 || data.is_null() {
                return None;
            }
            let v = if n > 0 && af == 32 {
                Some(*(data as *const c_ulong))
            } else {
                None
            };
            (self.xl.XFree)(data as *mut c_void);
            v
        }
    }

    fn active_window(&self) -> Option<xlib::Window> {
        self.prop_ulong(self.root, self.atom_active, xlib::XA_WINDOW)
            .filter(|&w| w != 0)
    }

    /// The active window and its direct children (games may render into a child).
    fn active_windows(&self) -> Vec<u32> {
        let Some(w) = self.active_window() else {
            return self.visible_toplevels();
        };
        let mut v = vec![w as u32];
        unsafe {
            let (mut root, mut parent) = (0, 0);
            let mut kids: *mut xlib::Window = std::ptr::null_mut();
            let mut n: c_uint = 0;
            if (self.xl.XQueryTree)(self.dpy, w, &mut root, &mut parent, &mut kids, &mut n) != 0
                && !kids.is_null()
            {
                for i in 0..n as usize {
                    v.push(*kids.add(i) as u32);
                }
                (self.xl.XFree)(kids as *mut c_void);
            }
        }
        v
    }

    /// Fallback without an EWMH window manager: every visible top-level window.
    fn visible_toplevels(&self) -> Vec<u32> {
        let mut v = Vec::new();
        unsafe {
            let (mut root, mut parent) = (0, 0);
            let mut kids: *mut xlib::Window = std::ptr::null_mut();
            let mut n: c_uint = 0;
            if (self.xl.XQueryTree)(
                self.dpy,
                self.root,
                &mut root,
                &mut parent,
                &mut kids,
                &mut n,
            ) != 0
                && !kids.is_null()
            {
                for i in 0..n as usize {
                    let w = *kids.add(i);
                    let mut a: xlib::XWindowAttributes = std::mem::zeroed();
                    if w != self.win
                        && (self.xl.XGetWindowAttributes)(self.dpy, w, &mut a) != 0
                        && a.map_state == xlib::IsViewable
                        && a.width > 64
                    {
                        v.push(w as u32);
                    }
                }
                (self.xl.XFree)(kids as *mut c_void);
            }
        }
        v
    }

    fn active_pid(&self) -> Option<u32> {
        self.prop_ulong(self.active_window()?, self.atom_pid, xlib::XA_CARDINAL)
            .map(|p| p as u32)
    }

    fn window_center(&self, w: xlib::Window) -> Option<(i32, i32)> {
        unsafe {
            let (mut rx, mut ry) = (0, 0);
            let mut child = 0;
            if (self.xl.XTranslateCoordinates)(
                self.dpy, w, self.root, 0, 0, &mut rx, &mut ry, &mut child,
            ) == 0
            {
                return None;
            }
            let mut a: xlib::XWindowAttributes = std::mem::zeroed();
            (self.xl.XGetWindowAttributes)(self.dpy, w, &mut a);
            Some((rx + a.width / 2, ry + a.height / 2))
        }
    }

    fn monitors(&self) -> Vec<(Rect, bool)> {
        let mut out = Vec::new();
        if let Some(xr) = &self.xrandr {
            unsafe {
                let mut n = 0;
                let m = (xr.XRRGetMonitors)(self.dpy, self.root, xlib::True, &mut n);
                if !m.is_null() {
                    for i in 0..n as usize {
                        let mi = &*m.add(i);
                        out.push((
                            Rect {
                                x: mi.x,
                                y: mi.y,
                                w: mi.width,
                                h: mi.height,
                            },
                            mi.primary != 0,
                        ));
                    }
                    (xr.XRRFreeMonitors)(m);
                }
            }
        }
        if out.is_empty() {
            unsafe {
                let s = (self.xl.XDefaultScreen)(self.dpy);
                let w = (self.xl.XDisplayWidth)(self.dpy, s);
                let h = (self.xl.XDisplayHeight)(self.dpy, s);
                out.push((Rect { x: 0, y: 0, w, h }, true));
            }
        }
        out
    }

    /// Xft.dpi / 96 — what desktops use for UI scaling on X11.
    fn dpi_scale(&self) -> f32 {
        unsafe {
            let p = (self.xl.XResourceManagerString)(self.dpy);
            if p.is_null() {
                return 1.0;
            }
            let s = std::ffi::CStr::from_ptr(p).to_string_lossy();
            s.lines()
                .find_map(|l| {
                    l.strip_prefix("Xft.dpi:")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                })
                .map(|d| (d / 96.0).clamp(0.5, 4.0))
                .unwrap_or(1.0)
        }
    }

    fn grab(&mut self, hk: &Hotkey) -> bool {
        self.ungrab();
        let Some(sym) = uo_common::hotkey::x11_keysym(&hk.key) else {
            return false;
        };
        unsafe {
            let code = (self.xl.XKeysymToKeycode)(self.dpy, sym as xlib::KeySym) as c_int;
            if code == 0 {
                return false;
            }
            let mut mods = 0;
            if hk.ctrl {
                mods |= xlib::ControlMask;
            }
            if hk.alt {
                mods |= xlib::Mod1Mask;
            }
            if hk.shift {
                mods |= xlib::ShiftMask;
            }
            if hk.win {
                mods |= xlib::Mod4Mask;
            }
            // Also grab with CapsLock / NumLock on, or the key "stops working".
            for extra in [
                0,
                xlib::LockMask,
                xlib::Mod2Mask,
                xlib::LockMask | xlib::Mod2Mask,
            ] {
                (self.xl.XGrabKey)(
                    self.dpy,
                    code,
                    mods | extra,
                    self.root,
                    xlib::True,
                    xlib::GrabModeAsync,
                    xlib::GrabModeAsync,
                );
            }
            (self.xl.XSync)(self.dpy, xlib::False);
            self.grabbed = Some((code, mods));
        }
        true
    }

    fn ungrab(&mut self) {
        if let Some((code, mods)) = self.grabbed.take() {
            unsafe {
                for extra in [
                    0,
                    xlib::LockMask,
                    xlib::Mod2Mask,
                    xlib::LockMask | xlib::Mod2Mask,
                ] {
                    (self.xl.XUngrabKey)(self.dpy, code, mods | extra, self.root);
                }
            }
        }
    }

    /// Drain pending X events; returns true if the hotkey was pressed.
    fn pump(&mut self) -> bool {
        let mut hot = false;
        unsafe {
            while (self.xl.XPending)(self.dpy) > 0 {
                let mut ev: xlib::XEvent = std::mem::zeroed();
                (self.xl.XNextEvent)(self.dpy, &mut ev);
                if ev.get_type() == xlib::KeyPress {
                    if let Some((code, _)) = self.grabbed {
                        if ev.key.keycode as c_int == code {
                            hot = true;
                        }
                    }
                }
            }
        }
        hot
    }

    /// Show the canvas. `dirty` (the graph between sensor ticks) limits the
    /// copy and the upload to that rectangle; any move/resize sends it all.
    fn present(&mut self, x: i32, y: i32, px: &[u32], w: i32, h: i32, dirty: Option<PxRect>) {
        unsafe {
            let mut dirty = dirty;
            if self.geom != (x, y, w, h) {
                (self.xl.XMoveResizeWindow)(self.dpy, self.win, x, y, w as c_uint, h as c_uint);
                self.geom = (x, y, w, h);
                dirty = None;
            }
            if !self.mapped {
                (self.xl.XMapRaised)(self.dpy, self.win);
                self.mapped = true;
                dirty = None;
            }
            if self.shm_present(px, w, h, dirty) {
                return;
            }
            let img = (self.xl.XCreateImage)(
                self.dpy,
                self.visual,
                self.depth as c_uint,
                xlib::ZPixmap,
                0,
                px.as_ptr() as *mut c_char,
                w as c_uint,
                h as c_uint,
                32,
                0,
            );
            if !img.is_null() {
                (self.xl.XPutImage)(
                    self.dpy,
                    self.win,
                    self.gc,
                    img,
                    0,
                    0,
                    0,
                    0,
                    w as c_uint,
                    h as c_uint,
                );
                (*img).data = std::ptr::null_mut(); // the pixels are ours
                (self.xl.XFree)(img as *mut c_void);
            }
            (self.xl.XFlush)(self.dpy);
        }
    }

    fn shm_free(&mut self) {
        let (Some(xs), Some(s)) = (&self.xshm, self.shm.take()) else {
            return;
        };
        unsafe {
            let mut info = s.info;
            (xs.XShmDetach)(self.dpy, &mut *info);
            (self.xl.XSync)(self.dpy, xlib::False);
            libc::shmdt(info.shmaddr as *const c_void);
            (*s.img).data = std::ptr::null_mut();
            (self.xl.XFree)(s.img as *mut c_void);
        }
    }

    fn shm_present(&mut self, px: &[u32], w: i32, h: i32, dirty: Option<PxRect>) -> bool {
        if self.xshm.is_none() {
            return false;
        }
        let mut dirty = dirty;
        if self.shm.as_ref().is_some_and(|s| s.w != w || s.h != h) {
            self.shm_free();
        }
        if self.shm.is_none() {
            dirty = None; // a fresh segment holds nothing yet
        }
        unsafe {
            if self.shm.is_none() {
                let xs = self.xshm.as_ref().unwrap();
                let mut info: Box<x11_dl::xshm::XShmSegmentInfo> = Box::new(std::mem::zeroed());
                let img = (xs.XShmCreateImage)(
                    self.dpy,
                    self.visual,
                    self.depth as c_uint,
                    xlib::ZPixmap,
                    std::ptr::null_mut(),
                    &mut *info,
                    w as c_uint,
                    h as c_uint,
                );
                if img.is_null() {
                    self.xshm = None;
                    return false;
                }
                let size = (*img).bytes_per_line as usize * h as usize;
                let id = libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600);
                let addr = if id >= 0 {
                    libc::shmat(id, std::ptr::null(), 0)
                } else {
                    usize::MAX as *mut c_void
                };
                if id < 0 || addr as isize == -1 {
                    (self.xl.XFree)(img as *mut c_void);
                    self.xshm = None;
                    return false;
                }
                info.shmid = id;
                info.shmaddr = addr as *mut c_char;
                info.readOnly = xlib::False;
                (*img).data = addr as *mut c_char;
                let ok = (xs.XShmAttach)(self.dpy, &mut *info) != 0;
                (self.xl.XSync)(self.dpy, xlib::False);
                libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()); // freed once both sides detach
                if !ok {
                    libc::shmdt(addr);
                    (*img).data = std::ptr::null_mut();
                    (self.xl.XFree)(img as *mut c_void);
                    self.xshm = None;
                    return false;
                }
                self.shm = Some(ShmImage { img, info, w, h });
            }
            let s = self.shm.as_ref().unwrap();
            let bpl = (*s.img).bytes_per_line as usize;
            let dst = (*s.img).data as *mut u8;
            let r = dirty.unwrap_or(PxRect { x: 0, y: 0, w, h });
            for row in r.y as usize..(r.y + r.h) as usize {
                std::ptr::copy_nonoverlapping(
                    px.as_ptr().add(row * w as usize + r.x as usize) as *const u8,
                    dst.add(row * bpl + r.x as usize * 4),
                    r.w as usize * 4,
                );
            }
            let xs = self.xshm.as_ref().unwrap();
            (xs.XShmPutImage)(
                self.dpy,
                self.win,
                self.gc,
                s.img,
                r.x,
                r.y,
                r.x,
                r.y,
                r.w as c_uint,
                r.h as c_uint,
                xlib::False,
            );
            // Wait for the server to read the segment before we draw into it again.
            (self.xl.XSync)(self.dpy, xlib::False);
        }
        true
    }

    fn raise(&self) {
        if self.mapped {
            unsafe {
                (self.xl.XRaiseWindow)(self.dpy, self.win);
                (self.xl.XFlush)(self.dpy);
            }
        }
    }

    fn hide(&mut self) {
        self.shm_free();
        if self.mapped {
            unsafe {
                (self.xl.XUnmapWindow)(self.dpy, self.win);
                (self.xl.XFlush)(self.dpy);
            }
            self.mapped = false;
        }
    }
}

// ---------------------------------------------------------------- tray

struct TrayModel {
    visible: bool,
    hotkey: String,
    layer: bool,
}

fn send(cmd: &'static str) -> Box<dyn Fn(&mut TrayModel) + Send + Sync> {
    Box::new(move |_| {
        ipc::send(cmd);
    })
}

impl ksni::Tray for TrayModel {
    fn id(&self) -> String {
        "universal-overlay".into()
    }

    fn title(&self) -> String {
        "Universal Overlay".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: format!("Universal Overlay — {}", self.hotkey),
            ..Default::default()
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 32, 48]
            .into_iter()
            .map(|s| ksni::Icon {
                width: s,
                height: s,
                data: icon_pixels(s)
                    .into_iter()
                    .flat_map(|p| p.to_be_bytes())
                    .collect(),
            })
            .collect()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        ipc::send("toggle");
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let item = |label: String, cmd: &'static str| {
            StandardItem {
                label,
                activate: send(cmd),
                ..Default::default()
            }
            .into()
        };
        let mut v = vec![
            item(
                format!(
                    "{}  ({})",
                    if self.visible {
                        "Hide overlay"
                    } else {
                        "Show overlay"
                    },
                    self.hotkey
                ),
                "toggle",
            ),
            item("Settings…".into(), "settings"),
            item("Reload config".into(), "reload"),
        ];
        v.push(if self.layer {
            item("Remove Vulkan layer".into(), "uninstall-layer")
        } else {
            item("Install Vulkan layer (for FPS)".into(), "install-layer")
        });
        v.push(ksni::MenuItem::Separator);
        v.push(item("Exit".into(), "quit"));
        v
    }
}

// ---------------------------------------------------------------- app

struct App {
    core: Core,
    x: X11,
    visible: bool,
    tray: Option<ksni::blocking::Handle<TrayModel>>,
    next_tick: Instant,
    next_frame: Instant,
    mon: (Rect, f32),
    quit: bool,
}

impl App {
    fn update_tray(&self) {
        if let Some(t) = &self.tray {
            let (v, hk, layer) = (
                self.visible,
                self.core.cfg.general.hotkey.clone(),
                frames::layer_manifest_installed().is_some(),
            );
            t.update(move |m| {
                m.visible = v;
                m.hotkey = hk;
                m.layer = layer;
            });
        }
    }

    fn register_hotkey(&mut self) {
        if super::wayland_shortcut::is_wayland() {
            // The compositor owns the keyboard: let the desktop run `--toggle`.
            self.x.ungrab();
            if let Err(e) = super::wayland_shortcut::sync(&self.core.cfg.general.hotkey) {
                eprintln!("universal-overlay: {e}");
            }
            return;
        }
        let ok = Hotkey::parse(&self.core.cfg.general.hotkey).is_some_and(|hk| self.x.grab(&hk));
        if !ok {
            eprintln!(
                "universal-overlay: hotkey {} is unavailable; bind `universal-overlay --toggle` in your desktop settings",
                self.core.cfg.general.hotkey
            );
        }
    }

    fn show(&mut self) {
        if self.visible {
            return;
        }
        self.visible = true;
        self.core.start_frames();
        let now = Instant::now();
        self.next_tick = now;
        self.next_frame = now;
        self.tick();
        self.update_tray();
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.x.hide();
        self.core.on_hidden();
        self.update_tray();
    }

    fn toggle(&mut self) {
        if self.visible {
            self.hide()
        } else {
            self.show()
        }
    }

    fn reload(&mut self) {
        if self.core.reload(self.visible) {
            self.register_hotkey();
        }
        if self.visible {
            self.tick();
        }
        self.update_tray();
    }

    fn pick_monitor(&self) -> (Rect, f32) {
        let mons = self.x.monitors();
        let dpi = self.x.dpi_scale();
        let primary = mons
            .iter()
            .find(|m| m.1)
            .or(mons.first())
            .map(|m| m.0)
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            });
        if self.core.cfg.general.monitor == MonitorMode::Foreground {
            if let Some((cx, cy)) = self.x.active_window().and_then(|w| self.x.window_center(w)) {
                if let Some(m) = mons
                    .iter()
                    .find(|(r, _)| cx >= r.x && cx < r.x + r.w && cy >= r.y && cy < r.y + r.h)
                {
                    return (m.0, dpi);
                }
            }
        }
        (primary, dpi)
    }

    fn tick(&mut self) {
        let pid = self.x.active_pid().filter(|&p| p != std::process::id());
        frames::set_target_windows(self.x.active_windows());
        self.core.tick(pid);
        self.mon = self.pick_monitor();
        self.render();
        self.x.raise();
    }

    fn render(&mut self) {
        if !self.visible {
            return;
        }
        let (mon, dpi) = self.mon;
        let (x, y) = self.core.render(mon, dpi, 1.0);
        let cv = &self.core.canvas;
        let (w, h) = (cv.w, cv.h);
        // Borrow dance: present needs &mut X11 and the canvas pixels.
        let px = std::mem::take(&mut self.core.canvas.px);
        self.x.present(x, y, &px, w, h, self.core.dirty);
        self.core.canvas.px = px;
    }

    fn command(&mut self, cmd: &str) {
        match cmd.trim() {
            "toggle" => self.toggle(),
            "show" => self.show(),
            "hide" => self.hide(),
            "reload" => self.reload(),
            "settings" => {
                let _ = std::process::Command::new(super::exe_dir().join(SETTINGS_EXE)).spawn();
            }
            "install-layer" => {
                if let Err(e) = frames::install_layer() {
                    eprintln!("universal-overlay: {e}");
                }
                self.core.stop_frames();
                if self.visible {
                    self.core.start_frames();
                }
                self.update_tray();
            }
            "uninstall-layer" => {
                let _ = frames::uninstall_layer();
                self.update_tray();
            }
            "quit" => self.quit = true,
            _ => {}
        }
    }

    fn timeout_ms(&self) -> c_int {
        if !self.visible {
            return -1; // sleep until an event arrives
        }
        let now = Instant::now();
        let mut next = self.next_tick;
        if self.core.cfg.graph.enabled {
            next = next.min(self.next_frame);
        }
        // Round up: rounding down would spin for the last sub-millisecond.
        next.saturating_duration_since(now)
            .as_micros()
            .div_ceil(1000)
            .min(10_000) as c_int
    }

    fn on_timers(&mut self) {
        if !self.visible {
            return;
        }
        let now = Instant::now();
        let g = &self.core.cfg.general;
        let tick_every = Duration::from_millis(g.update_interval_ms as u64);
        let frame_every = Duration::from_secs_f64(1.0 / g.graph_refresh_hz.max(1) as f64);
        if now >= self.next_tick {
            self.next_tick = now + tick_every;
            self.next_frame = now + frame_every;
            self.tick();
        } else if self.core.cfg.graph.enabled && now >= self.next_frame {
            self.next_frame = now + frame_every;
            self.render();
        }
    }
}

/// UUID of the GNOME Shell extension shipped in packaging/linux.
const GNOME_EXTENSION: &str = "universal-overlay@uo.local";

fn gnome_extension_enabled() -> bool {
    if !std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase()
        .contains("gnome")
    {
        return false;
    }
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(GNOME_EXTENSION))
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("--install-layer") {
        match frames::install_layer() {
            Ok(p) => println!("Vulkan layer installed: {}", p.display()),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        ipc::send("reload");
        return;
    }
    if has("--uninstall-layer") {
        let _ = frames::uninstall_layer();
        println!("Vulkan layer removed");
        return;
    }
    if has("--sync-shortcut") {
        let (cfg, _) = uo_common::config::Config::load();
        match super::wayland_shortcut::sync(&cfg.general.hotkey) {
            Ok(m) => println!("  Hotkey: {m}"),
            Err(e) => println!("  Hotkey: {e}"),
        }
        return;
    }
    if has("--remove-shortcut") {
        super::wayland_shortcut::remove();
        return;
    }
    for (flag, cmd) in [
        ("--quit", "quit"),
        ("--reload", "reload"),
        ("--show", "show"),
        ("--hide", "hide"),
    ] {
        if has(flag) {
            ipc::send(cmd);
            return;
        }
    }
    if has("--settings") {
        if !ipc::send("settings") {
            let _ = std::process::Command::new(super::exe_dir().join(SETTINGS_EXE)).spawn();
        }
        return;
    }
    // --toggle on a stopped overlay starts it, visible (handy for a desktop
    // shortcut or the GNOME extension).
    let toggle = has("--toggle");
    if toggle && ipc::send("toggle") {
        return;
    }
    if ipc::is_running() {
        // Launched again from the app menu: behave like reopening an app.
        ipc::send("reload");
        if !has("--background") {
            ipc::send("settings");
        }
        return;
    }

    let sock = ipc::socket_path();
    let _ = std::fs::remove_file(&sock);
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "universal-overlay: could not create {}: {e}",
                sock.display()
            );
            std::process::exit(1);
        }
    };
    let _ = listener.set_nonblocking(true);

    let x = match X11::open() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("universal-overlay: {e}");
            std::process::exit(1);
        }
    };
    let core = Core::new();
    let show = core.cfg.general.show_on_start || toggle;
    let hotkey = core.cfg.general.hotkey.clone();
    use ksni::blocking::TrayMethods;
    // On GNOME our shell extension shows the panel icon; a second
    // StatusNotifierItem (via the AppIndicator extension) would duplicate it.
    let tray = if gnome_extension_enabled() {
        None
    } else {
        TrayModel {
            visible: false,
            hotkey,
            layer: frames::layer_manifest_installed().is_some(),
        }
        .spawn()
        .ok()
    };
    let now = Instant::now();
    let mut app = App {
        core,
        x,
        visible: false,
        tray,
        next_tick: now,
        next_frame: now,
        mon: (
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            1.0,
        ),
        quit: false,
    };
    app.register_hotkey();
    if show {
        app.show();
    }

    let xfd = app.x.fd();
    let lfd = listener.as_raw_fd();
    while !app.quit {
        let mut fds = [
            libc::pollfd {
                fd: xfd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: lfd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        unsafe { libc::poll(fds.as_mut_ptr(), 2, app.timeout_ms()) };
        if app.x.pump() {
            app.toggle();
        }
        while let Ok((mut s, _)) = listener.accept() {
            let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            app.command(&buf);
        }
        app.on_timers();
    }
    app.hide();
    if let Some(t) = &app.tray {
        let _ = t.shutdown();
    }
    let _ = std::fs::remove_file(&sock);
}

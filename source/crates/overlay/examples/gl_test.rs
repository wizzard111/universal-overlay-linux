//! Linux test: an OpenGL (GLX) window swapping ~60 times per second, to
//! verify that the overlay measures OpenGL games through X Present.
//!   cargo run --release --example gl_test -- [secs]

fn main() {
    use std::ffi::{c_int, c_uint, CString};
    use std::time::{Duration, Instant};
    use x11_dl::{glx, xlib};

    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    unsafe {
        let xl = xlib::Xlib::open().expect("libX11");
        let gl = glx::Glx::open().expect("libGL");
        let dpy = (xl.XOpenDisplay)(std::ptr::null());
        assert!(!dpy.is_null(), "no X display");
        let screen = (xl.XDefaultScreen)(dpy);
        let root = (xl.XRootWindow)(dpy, screen);
        let mut attrs = [glx::GLX_RGBA, glx::GLX_DOUBLEBUFFER, 0];
        let vi = (gl.glXChooseVisual)(dpy, screen, attrs.as_mut_ptr());
        assert!(!vi.is_null(), "no GLX visual");
        let mut swa: xlib::XSetWindowAttributes = std::mem::zeroed();
        swa.colormap = (xl.XCreateColormap)(dpy, root, (*vi).visual, xlib::AllocNone);
        let win = (xl.XCreateWindow)(
            dpy,
            root,
            100,
            100,
            320,
            240,
            0,
            (*vi).depth,
            xlib::InputOutput as c_uint,
            (*vi).visual,
            xlib::CWColormap,
            &mut swa,
        );
        let title = CString::new("gl_test").unwrap();
        (xl.XStoreName)(dpy, win, title.as_ptr() as *mut _);
        (xl.XMapWindow)(dpy, win);
        let ctx = (gl.glXCreateContext)(dpy, vi, std::ptr::null_mut(), xlib::True);
        (gl.glXMakeCurrent)(dpy, win, ctx);
        let name = CString::new("glClearColor").unwrap();
        let clear_color: extern "C" fn(f32, f32, f32, f32) = std::mem::transmute(
            (gl.glXGetProcAddress)(name.as_ptr() as *const u8).expect("glClearColor"),
        );
        let name = CString::new("glClear").unwrap();
        let clear: extern "C" fn(c_uint) = std::mem::transmute(
            (gl.glXGetProcAddress)(name.as_ptr() as *const u8).expect("glClear"),
        );
        let name = CString::new("glGetString").unwrap();
        let get_string: extern "C" fn(c_uint) -> *const std::ffi::c_char = std::mem::transmute(
            (gl.glXGetProcAddress)(name.as_ptr() as *const u8).expect("glGetString"),
        );
        let r = get_string(0x1F01);
        if !r.is_null() {
            println!(
                "GL_RENDERER: {}",
                std::ffi::CStr::from_ptr(r).to_string_lossy()
            );
        }
        println!(
            "pid {} window 0x{:x}: GL swaps at ~60 FPS for {secs} s",
            std::process::id(),
            win
        );
        let start = Instant::now();
        let mut next = start;
        let mut n: c_int = 0;
        while start.elapsed() < Duration::from_secs(secs) {
            let t = start.elapsed().as_secs_f32();
            clear_color(t.sin() * 0.5 + 0.5, 0.2, 0.3, 1.0);
            clear(0x4000); // GL_COLOR_BUFFER_BIT
            (gl.glXSwapBuffers)(dpy, win);
            n += 1;
            next += Duration::from_micros(16_667);
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
        println!("swapped {n} frames");
        (gl.glXMakeCurrent)(dpy, 0, std::ptr::null_mut());
        (gl.glXDestroyContext)(dpy, ctx);
        (xl.XDestroyWindow)(dpy, win);
        (xl.XCloseDisplay)(dpy);
    }
}

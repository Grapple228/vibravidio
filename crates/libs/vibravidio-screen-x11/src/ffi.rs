#![allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    non_upper_case_globals
)]

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};

pub type Display = c_void;
pub type Window = c_ulong;
pub type Drawable = c_ulong;
pub type Visual = c_void;

pub const ZPixmap: c_int = 2;
pub const AllPlanes: c_ulong = !0;
pub const IPC_PRIVATE: c_int = 0;
pub const IPC_CREAT: c_int = 0o1000;
pub const IPC_RMID: c_int = 0;

#[repr(C)]
pub struct XWindowAttributes {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub depth: c_int,
    pub visual: *mut Visual,
    pub root: Window,
    pub class: c_int,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: c_int,
    pub colormap: c_ulong,
    pub map_installed: c_int,
    pub map_state: c_int,
    pub all_event_masks: c_long,
    pub your_event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: c_int,
    pub screen: *mut c_void,
}

#[repr(C)]
pub struct XShmSegmentInfo {
    pub shmseg: c_ulong,
    pub shmid: c_int,
    pub shmaddr: *mut u8,
    pub readOnly: c_int,
}

/// Частичный `XImage` — только поля до `obdata` (включительно).
///
/// Мы НЕ вызываем `XDestroyImage`, поэтому таблица `f` (function pointers)
/// нам не нужна — освобождаем структуру через `XFree`. Поля до `bytes_per_line`
/// расположены строго как в Xlib 1.6+:
///   width(0) height(4) xoffset(8) format(12) data(16) byte_order(20)
///   bitmap_unit(24) bitmap_bit_order(28) bitmap_pad(32) depth(36)
///   bytes_per_line(40) bits_per_pixel(44) red_mask(48) green_mask(56)
///   blue_mask(64) obdata(72)
#[repr(C)]
pub struct XImage {
    pub width: c_int,
    pub height: c_int,
    pub xoffset: c_int,
    pub format: c_int,
    pub data: *mut u8,
    pub byte_order: c_int,
    pub bitmap_unit: c_int,
    pub bitmap_bit_order: c_int,
    pub bitmap_pad: c_int,
    pub depth: c_int,
    pub bytes_per_line: c_int,
    pub bits_per_pixel: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub obdata: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XErrorEvent {
    pub typ: c_int,
    pub display: *mut Display,
    pub resourceid: c_ulong,
    pub serial: c_ulong,
    pub error_code: u8,
    pub request_code: u8,
    pub minor_code: u8,
}

pub type XErrorHandler = Option<unsafe extern "C" fn(*mut Display, *mut XErrorEvent) -> c_int>;

// X11-функции: libX11 (XOpenDisplay, XCloseDisplay, XFree, XGetWindowAttributes, ...)
// XShm*: libXext
#[link(name = "X11")]
#[link(name = "Xext")]
unsafe extern "C" {
    pub fn XInitThreads() -> c_int;
    pub fn XOpenDisplay(name: *const c_char) -> *mut Display;
    pub fn XCloseDisplay(dpy: *mut Display) -> c_int;
    pub fn XDefaultRootWindow(dpy: *mut Display) -> Window;
    pub fn XGetWindowAttributes(
        dpy: *mut Display,
        w: Window,
        attrs: *mut XWindowAttributes,
    ) -> c_int;

    pub fn XFree(data: *mut c_void) -> c_int;

    pub fn XShmQueryExtension(dpy: *mut Display) -> c_int;
    pub fn XShmCreateImage(
        dpy: *mut Display,
        visual: *mut Visual,
        depth: c_uint,
        format: c_int,
        data: *mut u8,
        shminfo: *mut XShmSegmentInfo,
        width: c_uint,
        height: c_uint,
    ) -> *mut XImage;
    pub fn XShmAttach(dpy: *mut Display, shminfo: *mut XShmSegmentInfo) -> c_int;
    pub fn XShmDetach(dpy: *mut Display, shminfo: *mut XShmSegmentInfo) -> c_int;
    pub fn XShmGetImage(
        dpy: *mut Display,
        d: Drawable,
        img: *mut XImage,
        x: c_int,
        y: c_int,
        plane_mask: c_ulong,
    ) -> c_int;

    /// Устанавливает глобальный error handler Xlib. Возвращает предыдущий.
    /// По умолчанию Xlib на любую X-ошибку делает exit(1).
    pub fn XSetErrorHandler(handler: XErrorHandler) -> XErrorHandler;

    /// Синхронный roundtrip: ждёт, пока сервер обработает все запросы,
    /// и вызывает error handler для всех накопленных ошибок.
    /// `discard: Bool` — 0 (False), чтобы ошибки не терялись.
    pub fn XSync(dpy: *mut Display, discard: c_int) -> c_int;
}

// SysV shm — часть libc, но на некоторых системах (musl, старые glibc)
// не всё линкуется автоматически. -lrt на современных glibc уже не нужен,
// но пусть будет явно для musl/alpine.
unsafe extern "C" {
    pub fn shmget(key: c_int, size: usize, flag: c_int) -> c_int;
    pub fn shmat(shmid: c_int, addr: *const c_void, flag: c_int) -> *mut c_void;
    pub fn shmdt(addr: *const c_void) -> c_int;
    pub fn shmctl(shmid: c_int, cmd: c_int, buf: *mut c_void) -> c_int;
}

use std::cell::Cell;
use std::ffi::{c_int, c_uint};
use std::ptr::NonNull;
use std::sync::Once;
use std::time::{Duration, Instant};

use vibravidio_core::{
    CaptureSource, CaptureSourceKind, CapturedFrame, ColorSpace, Error, FrameInfo, PixelFormat,
    Result, ScreenBackend, SourceDiscovery, VideoBackend, VideoConfig,
};

use crate::ffi;

// ============================================================================
// Xlib error handler
// ============================================================================
//
// Xlib по умолчанию делает `exit(1)` при любой X-ошибке. `XShmAttach`
// отправляет запрос асинхронно — `BadAccess` (например, ssh -X без trusted
// forwarding, или сервер с запретом MIT-SHM) убивает процесс ДО того, как
// мы проверим return value. Единственный способ — установить свой handler.
//
// Handler глобальный на процесс (`XSetErrorHandler` не per-thread), но
// вызывается он в контексте потока, сделавшего Xlib-вызов. Сохраняем в
// thread-local — так параллельные тесты не затирают друг другу ошибки.

thread_local! {
    static LAST_X_ERROR: Cell<Option<ffi::XErrorEvent>> = const { Cell::new(None) };
}

unsafe extern "C" fn x_error_handler(
    _dpy: *mut ffi::Display,
    event: *mut ffi::XErrorEvent,
) -> c_int {
    if !event.is_null() {
        let ev = unsafe { *event };
        LAST_X_ERROR.with(|cell| cell.set(Some(ev)));
    }
    // Xlib игнорирует return value, но контракт требует c_int.
    0
}

fn clear_x_errors() {
    LAST_X_ERROR.with(|cell| cell.set(None));
}

fn take_x_error() -> Option<ffi::XErrorEvent> {
    LAST_X_ERROR.with(|cell| cell.take())
}

// ============================================================================
// Initialization (Once)
// ============================================================================

static XLIB_INIT: Once = Once::new();

/// `XInitThreads` + `XSetErrorHandler` — оба обязаны выполниться до
/// любого содержательного Xlib-вызова.
fn ensure_xlib_ready() -> Result<()> {
    let mut ok = true;
    XLIB_INIT.call_once(|| {
        ok = unsafe { ffi::XInitThreads() } != 0;
        if ok {
            // Сохраняем предыдущий handler? Нет — Xlib не даёт способа
            // «сцепить» их. Установка нашего handler'а — это компромисс:
            // мы получаем контроль над своими X-ошибками, но перезаписываем
            // чужой (winit/eframe). На практике screen-backend открывается
            // до/независимо от winit, и BadAccess на XShmAttach для нас
            // важнее чужого handler'а.
            unsafe { ffi::XSetErrorHandler(Some(x_error_handler)) };
        }
    });
    if ok {
        Ok(())
    } else {
        Err(Error::Ffi {
            code: -1,
            message: "XInitThreads failed",
        })
    }
}

// ============================================================================
// SHM probe — проверяет, что MIT-SHM реально работает на этом сервере
// ============================================================================

/// `XShmQueryExtension` возвращает `true` даже когда сервер не разрешает
/// attach (ssh -X, untrusted, политики). Пробный attach-detach — единственный
/// надёжный тест.
///
/// Внимание: probe делается ДО установки основного shm-сегмента в backend,
/// поэтому при неудаче `open_sync` вернёт `UnsupportedFormat`, и `Screen::run`
/// откажется стартовать до создания потока.
fn probe_shm(display: *mut ffi::Display) -> bool {
    const PROBE_SIZE: usize = 4096;

    clear_x_errors();

    let shmid = unsafe { ffi::shmget(ffi::IPC_PRIVATE, PROBE_SIZE, ffi::IPC_CREAT | 0o666) };
    if shmid < 0 {
        return false;
    }
    let addr = unsafe { ffi::shmat(shmid, std::ptr::null(), 0) };
    if addr as isize == -1 {
        unsafe { ffi::shmctl(shmid, ffi::IPC_RMID, std::ptr::null_mut()) };
        return false;
    }

    let mut shm = ffi::XShmSegmentInfo {
        shmseg: 0,
        shmid,
        shmaddr: addr as *mut u8,
        readOnly: 0,
    };

    let attach_ok = unsafe { ffi::XShmAttach(display, &mut shm) } != 0;
    // XShmAttach асинхронный. Без XSync BadAccess придёт позже — и убьёт
    // процесс, если мы не успели проверить handler.
    unsafe { ffi::XSync(display, 0) };
    let err = take_x_error();

    let success = attach_ok && err.is_none();

    if success {
        clear_x_errors();
        unsafe { ffi::XShmDetach(display, &mut shm) };
        unsafe { ffi::XSync(display, 0) };
        let _ = take_x_error();
    } else if let Some(ev) = err {
        tracing::debug!(
            "probe_shm: BadAccess on XShmAttach (error_code={}, req={})",
            ev.error_code,
            ev.request_code
        );
    }

    unsafe { ffi::shmdt(addr) };
    unsafe { ffi::shmctl(shmid, ffi::IPC_RMID, std::ptr::null_mut()) };

    success
}

// ============================================================================
// Backend
// ============================================================================

pub struct X11ScreenBackend {
    display: *mut ffi::Display,
    root: ffi::Window,

    shm: Box<ffi::XShmSegmentInfo>,
    ximage: Option<NonNull<ffi::XImage>>,

    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
    color: ColorSpace,
    seq: u64,
    start: Instant,

    frame_duration: Duration,
    last_capture: Instant,
}

unsafe impl Send for X11ScreenBackend {}

impl X11ScreenBackend {
    /// Sync open. Вызывающий сам решает, когда: `Screen::run` (тесты),
    /// `DefaultScreenBackend::open_sync` (fallback), `open_source` (async path
    /// внутри ready-future). Не в трейте — inherent, как договорились.
    pub fn open_sync(name: &str) -> Result<Self> {
        ensure_xlib_ready()?;

        let cname = if name == "default" {
            None
        } else {
            Some(std::ffi::CString::new(name).map_err(|_| Error::InvalidConfig)?)
        };
        let display =
            unsafe { ffi::XOpenDisplay(cname.as_ref().map_or(std::ptr::null(), |s| s.as_ptr())) };
        if display.is_null() {
            return Err(Error::DeviceNotFound);
        }

        if unsafe { ffi::XShmQueryExtension(display) } == 0 {
            unsafe { ffi::XCloseDisplay(display) };
            return Err(Error::UnsupportedFormat);
        }

        // Probe: extension declared ≠ разрешён attach. Проверяем реально.
        if !probe_shm(display) {
            unsafe { ffi::XCloseDisplay(display) };
            return Err(Error::UnsupportedFormat);
        }

        let root = unsafe { ffi::XDefaultRootWindow(display) };

        Ok(Self {
            display,
            root,
            shm: Box::new(ffi::XShmSegmentInfo {
                shmseg: 0,
                shmid: 0,
                shmaddr: std::ptr::null_mut(),
                readOnly: 0,
            }),
            ximage: None,
            width: 0,
            height: 0,
            stride: 0,
            format: PixelFormat::Bgra8,
            color: ColorSpace::Srgb,
            seq: 0,
            start: Instant::now(),
            frame_duration: Duration::from_nanos(1_000_000_000 / 30),
            last_capture: Instant::now(),
        })
    }

    fn release(&mut self) {
        if let Some(img) = self.ximage.take() {
            clear_x_errors();
            unsafe {
                ffi::XShmDetach(self.display, self.shm.as_mut());
                ffi::XFree(img.as_ptr() as *mut _);
            }
            unsafe { ffi::XSync(self.display, 0) };
            let _ = take_x_error();
        }
        if !self.shm.shmaddr.is_null() {
            unsafe { ffi::shmdt(self.shm.shmaddr as *const _) };
            self.shm.shmaddr = std::ptr::null_mut();
        }
    }

    /// Освобождает частично-инициализированное состояние после ошибки в
    /// `configure`. Обнуляет `self.shm`, чтобы `Drop::release` не сделал
    /// повторный `shmdt` на уже отцепленный адрес (SIGSEGV).
    fn cleanup_partial(&mut self, img: *mut ffi::XImage, addr: *mut u8, shmid: c_int) {
        unsafe {
            if !img.is_null() {
                ffi::XFree(img as *mut _);
            }
            if !addr.is_null() {
                ffi::shmdt(addr as *const _);
            }
            if shmid >= 0 {
                ffi::shmctl(shmid, ffi::IPC_RMID, std::ptr::null_mut());
            }
        }
        self.shm.shmaddr = std::ptr::null_mut();
        self.shm.shmid = 0;
        self.ximage = None;
    }
}

impl ScreenBackend for X11ScreenBackend {
    fn discover() -> Result<SourceDiscovery> {
        // MVP: только root window. Полный список мониторов/окон (XRandR /
        // XQueryTree + превью) — отдельным этапом, здесь только один источник.
        Ok(SourceDiscovery::Direct(vec![CaptureSource {
            id: "x11:root".into(),
            name: "Entire screen".into(),
            kind: CaptureSourceKind::Monitor,
            thumbnail: None,
        }]))
    }

    fn open_source(source: &CaptureSource) -> Result<Self> {
        let id = source.id.clone();
        match id.as_str() {
            "x11:root" | "default" => Self::open_sync("default"),
            _ => Err(Error::InvalidConfig),
        }
    }
}

impl VideoBackend for X11ScreenBackend {
    fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        self.release();

        let mut attrs: ffi::XWindowAttributes = unsafe { std::mem::zeroed() };
        if unsafe { ffi::XGetWindowAttributes(self.display, self.root, &mut attrs) } == 0 {
            return Err(Error::InvalidConfig);
        }

        let depth = attrs.depth as c_uint;
        let format = match depth {
            24 | 32 => PixelFormat::Bgra8,
            _ => return Err(Error::UnsupportedFormat),
        };

        let width = attrs.width as u32;
        let height = attrs.height as u32;
        if width == 0 || height == 0 {
            return Err(Error::InvalidConfig);
        }
        let stride = width * 4;
        let shm_size = (stride * height) as usize;

        let shmid = unsafe { ffi::shmget(ffi::IPC_PRIVATE, shm_size, ffi::IPC_CREAT | 0o666) };
        if shmid < 0 {
            return Err(Error::InvalidConfig);
        }
        let addr = unsafe { ffi::shmat(shmid, std::ptr::null(), 0) };
        if addr as isize == -1 {
            unsafe { ffi::shmctl(shmid, ffi::IPC_RMID, std::ptr::null_mut()) };
            return Err(Error::InvalidConfig);
        }

        // С этого момента любой error return ОБЯЗАН пройти через cleanup_partial.
        self.shm.shmid = shmid;
        self.shm.shmaddr = addr as *mut u8;

        let img = unsafe {
            ffi::XShmCreateImage(
                self.display,
                attrs.visual,
                depth,
                ffi::ZPixmap,
                self.shm.shmaddr,
                self.shm.as_mut(),
                width,
                height,
            )
        };
        if img.is_null() {
            self.cleanup_partial(std::ptr::null_mut(), addr as *mut u8, shmid);
            return Err(Error::InvalidConfig);
        }

        if unsafe { (*img).bytes_per_line as u32 } != stride {
            self.cleanup_partial(img, addr as *mut u8, shmid);
            return Err(Error::InvalidConfig);
        }

        // XShmAttach: sync + error check, потому что BadAccess асинхронный.
        clear_x_errors();
        let attach_ok = unsafe { ffi::XShmAttach(self.display, self.shm.as_mut()) } != 0;
        unsafe { ffi::XSync(self.display, 0) };
        let err = take_x_error();

        if !attach_ok || err.is_some() {
            self.cleanup_partial(img, addr as *mut u8, shmid);
            if let Some(ev) = err {
                tracing::warn!(
                    "XShmAttach failed: error_code={}, req={}",
                    ev.error_code,
                    ev.request_code
                );
            }
            return Err(Error::InvalidConfig);
        }

        // Авто-удаление когда все attachor'ы отвалятся.
        unsafe { ffi::shmctl(shmid, ffi::IPC_RMID, std::ptr::null_mut()) };

        self.ximage = Some(NonNull::new(img).unwrap());
        self.width = width;
        self.height = height;
        self.stride = stride;
        self.format = format;
        self.color = ColorSpace::Srgb;

        let fps = config.fps.max(1) as u64;
        self.frame_duration = Duration::from_nanos(1_000_000_000 / fps);
        self.last_capture = Instant::now() - self.frame_duration;
        Ok(())
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        let Some(img) = self.ximage else {
            return Ok(None);
        };

        if self.last_capture.elapsed() < self.frame_duration {
            return Ok(None);
        }
        self.last_capture = Instant::now();

        // XShmGetImage асинхронный. Без XSync ошибка придёт позже —
        // либо убьёт процесс, либо «прилипнет» к следующему вызову.
        // XSync на каждом кадре дорого (30 fps × микросекунды roundtrip),
        // но XShmGetImage уже сам блокирующий roundtrip по дизайну
        // (сервер должен скопировать данные в shm до возврата).
        clear_x_errors();
        let ok = unsafe {
            ffi::XShmGetImage(self.display, self.root, img.as_ptr(), 0, 0, ffi::AllPlanes)
        };
        if ok == 0 {
            let _ = take_x_error();
            return Ok(None);
        }
        // Ошибку, если она была, поймаем здесь; без XSync её просто не будет.
        if let Some(ev) = take_x_error() {
            tracing::warn!(
                "XShmGetImage error: code={}, req={}",
                ev.error_code,
                ev.request_code
            );
            return Ok(None);
        }
        self.seq += 1;

        let info = FrameInfo {
            width: self.width,
            height: self.height,
            stride: self.stride,
            format: self.format,
            color: self.color,
            sequence: self.seq,
            timestamp_ns: self.start.elapsed().as_nanos() as u64,
        };
        let data = unsafe {
            std::slice::from_raw_parts(self.shm.shmaddr, (self.stride * self.height) as usize)
        };
        Ok(Some(CapturedFrame { data, info }))
    }

    fn reset(&mut self) -> Result<()> {
        self.release();
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.release();
        Ok(())
    }
}

impl Drop for X11ScreenBackend {
    fn drop(&mut self) {
        self.release();
        if !self.display.is_null() {
            unsafe { ffi::XCloseDisplay(self.display) };
            self.display = std::ptr::null_mut();
        }
    }
}

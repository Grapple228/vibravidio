use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use vibravidio_core::{Error, OwnedFrame, Result, SourceKind, VideoBackend, VideoConfig};
use vibravidio_ringbuffer::BufferWriter;
use vibravidio_thread::Priority;
use vibravidio_thread::thread::{self, JoinHandle};

use crate::platform::DefaultScreenBackend;

pub struct Screen {
    config: VideoConfig,
    handle: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    poll_interval: Duration,
}

impl Screen {
    pub fn new(config: VideoConfig) -> Result<Self> {
        Ok(Self {
            config,
            handle: None,
            running: Arc::new(AtomicBool::new(false)),
            poll_interval: Duration::from_micros(500),
        })
    }

    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    pub fn run<const N: usize>(
        &mut self,
        writer: BufferWriter<N, OwnedFrame>,
        priority: Priority,
        #[cfg(target_os = "windows")] mmcss_value: vibravidio_thread::MmcssValue,
    ) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(Error::AlreadyRunning);
        }

        let mut backend = DefaultScreenBackend::open("default", SourceKind::Screen)?;
        backend.configure(&self.config)?;

        // Флаг ДО spawn: поток проверяет его первой инструкцией в цикле.
        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let poll = self.poll_interval;

        let handle = thread::spawn(move || {
            let _handle = vibravidio_thread::configure_thread(
                priority,
                #[cfg(target_os = "windows")]
                mmcss_value,
            );

            while running.load(Ordering::SeqCst) {
                match backend.capture_frame() {
                    Ok(Some(captured)) => {
                        let frame = OwnedFrame::from_captured(&captured);
                        drop(captured);
                        if writer.write_frame(frame).is_err() {
                            tracing::warn!("screen ring full, dropping frame");
                        }
                    }
                    Ok(None) => {
                        thread::sleep(poll);
                    }
                    Err(Error::SizeChanged { old, new }) => {
                        tracing::warn!("screen size {old:?} -> {new:?}");
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("❌ Screen capture error: {e}");
                        break;
                    }
                }
            }
        });

        self.handle = Some(handle);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use vibravidio_core::{ColorSpace, PixelFormat, VideoConfig};
    use vibravidio_ringbuffer::create_pair;
    use vibravidio_thread::Priority;

    // ========================================================================
    // Environment detection
    // ========================================================================

    /// Wayland-сессия? XWayland выставляет `DISPLAY`, но root window там
    /// содержит только X11-окна, а реальный desktop — Wayland-поверхность.
    /// Плюс: Wayland screen capture требует интерактивного
    /// xdg-desktop-portal диалога — в `cargo test` это зависание.
    fn is_wayland_session() -> bool {
        std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var("WAYLAND_DISPLAY").is_ok()
    }

    /// X11 есть только если задан `DISPLAY` и мы НЕ под Wayland
    /// (XWayland root пуст — тесты screen capture бессмысленны).
    fn has_x11_display() -> bool {
        !is_wayland_session() && std::env::var("DISPLAY").is_ok_and(|v| !v.is_empty())
    }

    /// Реальный X11 probe: открывает backend через "default".
    /// Вызывать только после `has_x11_display()` — иначе под Wayland
    /// пойдёт каскад на Wayland-портал и покажет диалог.
    fn open_screen_for_probe() -> bool {
        DefaultScreenBackend::open("default", SourceKind::Screen).is_ok()
    }

    /// Skip если screen capture недоступен в текущем окружении.
    /// Под Wayland — ВСЕГДА skip: интерактивный портал + пустой XWayland root.
    macro_rules! skip_if_no_screen {
        () => {
            if is_wayland_session() {
                tracing::debug!(
                    "skipping: Wayland session — screen capture requires \
                     interactive xdg-desktop-portal dialog; use `cargo test -- --ignored \
                     --test-threads=1 wayland_screen_capture_manual` for manual test"
                );
                return;
            }
            if !has_x11_display() || !open_screen_for_probe() {
                tracing::debug!("skipping: X11 screen not available");
                return;
            }
        };
    }

    // ========================================================================
    // Tests without hardware (always run)
    // ========================================================================

    #[test]
    fn screen_new_does_not_open_display() {
        // Screen::new только сохраняет config — backend не открывается,
        // портал не вызывается, Xlib не трогается.
        let config = VideoConfig::new(640, 480, 15, PixelFormat::Bgra8);
        let screen = Screen::new(config);
        assert!(screen.is_ok());
    }

    #[test]
    fn run_fails_without_display() {
        // Под Wayland `run()` откроет портал — интерактивно, тест зависнет.
        // Под X11 проверяем контракт: нет DISPLAY → Err.
        if is_wayland_session() {
            tracing::debug!("skipping: Wayland portal is interactive");
            return;
        }
        if has_x11_display() && open_screen_for_probe() {
            tracing::debug!("skipping: X11 screen available");
            return;
        }

        let config = VideoConfig::new(640, 480, 15, PixelFormat::Bgra8);
        let mut screen = Screen::new(config).unwrap();
        let (w, _r) = create_pair::<4, OwnedFrame>();
        let r = screen.run(
            w,
            Priority::Normal,
            #[cfg(target_os = "windows")]
            vibravidio_thread::MmcssValue::Video,
        );
        assert!(r.is_err());
    }

    // ========================================================================
    // Tests with X11 screen (skip on Wayland)
    // ========================================================================

    #[test]
    fn full_pipeline_receives_frames() {
        skip_if_no_screen!();

        // fps=60 → frame_duration ≈ 16ms, быстрее набираем кадры.
        let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
        let mut screen = Screen::new(config).unwrap();
        let (writer, reader) = create_pair::<8, OwnedFrame>();

        screen
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run");

        let start = Instant::now();
        let mut frames: Vec<OwnedFrame> = Vec::new();
        while start.elapsed() < Duration::from_secs(2) && frames.len() < 5 {
            if let Some(f) = reader.read_frame() {
                frames.push(f);
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        screen.stop();

        assert!(!frames.is_empty(), "must receive frames");

        for (i, f) in frames.iter().enumerate() {
            assert!(f.info.width > 0);
            assert!(f.info.height > 0);
            assert_eq!(
                f.info.stride,
                f.info.width * 4,
                "BGRA stride must equal width*4"
            );
            assert_eq!(f.data.len(), (f.info.stride * f.info.height) as usize);
            assert_eq!(f.info.format, PixelFormat::Bgra8);
            if i > 0 {
                assert!(f.info.sequence > frames[i - 1].info.sequence);
            }
        }
    }

    #[test]
    fn format_is_always_bgra() {
        skip_if_no_screen!();

        // config.format=Yuyv игнорируется X11Backend — всегда BGRA на 24/32bpp.
        let config = VideoConfig::new(640, 480, 60, PixelFormat::Yuyv);
        let mut screen = Screen::new(config).unwrap();
        let (writer, reader) = create_pair::<4, OwnedFrame>();

        screen
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run");

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut frame: Option<OwnedFrame> = None;
        while Instant::now() < deadline && frame.is_none() {
            frame = reader.read_frame();
            if frame.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        screen.stop();

        let f = frame.expect("must receive at least one frame");
        assert_eq!(f.info.format, PixelFormat::Bgra8);
        assert_eq!(f.info.color, ColorSpace::Srgb);
    }

    #[test]
    fn stop_terminates_producer() {
        skip_if_no_screen!();

        let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
        let mut screen = Screen::new(config).unwrap();
        let (writer, reader) = create_pair::<4, OwnedFrame>();

        screen
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run");

        std::thread::sleep(Duration::from_millis(200));
        screen.stop();

        // Дренируем то, что producer успел положить до stop.
        while reader.read_frame().is_some() {}

        // После stop новых быть не должно.
        std::thread::sleep(Duration::from_millis(200));
        assert!(reader.read_frame().is_none());
    }

    #[test]
    fn ring_overflow_drops_frame() {
        skip_if_no_screen!();

        // N=1 — почти гарантированно переполнится за 500ms при 60fps.
        let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
        let mut screen = Screen::new(config).unwrap();
        let (writer, reader) = create_pair::<1, OwnedFrame>();

        screen
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run");

        std::thread::sleep(Duration::from_millis(500));

        let mut count = 0;
        while reader.read_frame().is_some() {
            count += 1;
            if count > 100 {
                break;
            }
        }
        screen.stop();

        assert!(count >= 1, "must read at least one frame even with N=1");
    }

    #[test]
    fn drop_stops_thread() {
        skip_if_no_screen!();

        let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
        {
            let mut screen = Screen::new(config).unwrap();
            let (w, _r) = create_pair::<4, OwnedFrame>();
            screen
                .run(
                    w,
                    Priority::Normal,
                    #[cfg(target_os = "windows")]
                    vibravidio_thread::MmcssValue::Video,
                )
                .expect("run");
            std::thread::sleep(Duration::from_millis(150));
        }
        // Drop → stop → join. Если зависнет — тест повиснет.
    }

    #[test]
    fn data_size_matches_frame_info() {
        skip_if_no_screen!();

        let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
        let mut screen = Screen::new(config).unwrap();
        let (writer, reader) = create_pair::<4, OwnedFrame>();

        screen
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run");

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut frame: Option<OwnedFrame> = None;
        while Instant::now() < deadline && frame.is_none() {
            frame = reader.read_frame();
            if frame.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        screen.stop();

        let f = frame.expect("must receive frame");
        assert_eq!(f.info.format, PixelFormat::Bgra8);
        assert_eq!(f.info.stride, f.info.width * 4);
        assert_eq!(f.data.len(), (f.info.stride * f.info.height) as usize);
        assert!(f.info.timestamp_ns > 0);
    }

    #[test]
    fn double_run_returns_already_running() {
        skip_if_no_screen!();

        let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
        let mut screen = Screen::new(config).unwrap();
        let (w1, _r1) = create_pair::<4, OwnedFrame>();

        if screen
            .run(
                w1,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .is_err()
        {
            return;
        }

        let (w2, _r2) = create_pair::<4, OwnedFrame>();
        let r = screen.run(
            w2,
            Priority::Normal,
            #[cfg(target_os = "windows")]
            vibravidio_thread::MmcssValue::Video,
        );
        assert!(matches!(r, Err(Error::AlreadyRunning)));

        screen.stop();
    }

    // ========================================================================
    // Manual Wayland test (opt-in)
    // ========================================================================

    /// Ручной тест Wayland-бэкенда. Запуск:
    ///
    /// ```bash
    /// cargo test -r -p vibravidio -- \
    ///     --ignored --test-threads=1 wayland_screen_capture_manual
    /// ```
    ///
    /// Покажет xdg-desktop-portal диалог выбора монитора/окна. Выбери
    /// источник. Тест ждёт первый кадр до 15 секунд.
    ///
    /// Не идёт в обычном `cargo test` (пометка `#[ignore]`): портал
    /// интерактивный, в CI/headless тест зависнет.
    #[test]
    #[ignore = "requires interactive xdg-desktop-portal dialog; run manually"]
    fn wayland_screen_capture_manual() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug,vibravidio_screen_wayland=trace")
            .with_test_writer()
            .try_init();

        if !is_wayland_session() {
            tracing::debug!("skipping: not a Wayland session");
            return;
        }

        let config = VideoConfig::new(1920, 1080, 30, PixelFormat::Bgra8);
        let mut screen = Screen::new(config).unwrap();
        let (writer, reader) = create_pair::<4, OwnedFrame>();

        screen
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run — должно открыться окно выбора монитора в портале");

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut frame: Option<OwnedFrame> = None;
        while Instant::now() < deadline && frame.is_none() {
            frame = reader.read_frame();
            if frame.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        screen.stop();

        let f = frame.expect("must receive frame from Wayland portal");
        assert_eq!(f.info.format, PixelFormat::Bgra8);
        assert!(f.info.width > 0 && f.info.height > 0);
        assert!(
            f.data.iter().any(|&b| b != 0),
            "buffer must be non-empty (all-zero means portal returned a black frame)"
        );
    }
}

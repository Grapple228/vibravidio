use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use vibravidio_core::{Error, OwnedFrame, Result, VideoBackend, VideoConfig};
use vibravidio_ringbuffer::BufferWriter;
use vibravidio_thread::Priority;
use vibravidio_thread::thread::{self, JoinHandle};

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

    /// Backend открыт снаружи в `MediaController::worker`:
    /// - Linux/Wayland → `DefaultScreenBackend::open_source_async().await`
    /// - Linux/X11, Windows → `DefaultScreenBackend::open_source()` (sync)
    ///
    /// Capture loop — sync, ничего async внутри.
    pub fn run_with<const N: usize, B: VideoBackend>(
        &mut self,
        mut backend: B,
        writer: BufferWriter<N, OwnedFrame>,
        priority: Priority,
        #[cfg(target_os = "windows")] mmcss_value: vibravidio_thread::MmcssValue,
    ) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(Error::AlreadyRunning);
        }

        backend.configure(&self.config)?;

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
    use vibravidio_core::{PixelFormat, VideoConfig};

    // ========================================================================
    // Always-run test (no backend needed)
    // ========================================================================

    #[test]
    fn screen_new_does_not_open_display() {
        let config = VideoConfig::new(640, 480, 15, PixelFormat::Bgra8);
        let screen = Screen::new(config);
        assert!(screen.is_ok());
    }

    // ========================================================================
    // Linux-only tests (X11 / Wayland)
    // ========================================================================
    //
    // На Windows `DefaultScreenBackend` — `WindowsScreenBackend` со stub'ом,
    // никакого `open_source_async` не существует. Все тесты с реальным
    // backend'ом — под `#[cfg(target_os = "linux")]`.

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;
        use std::time::Instant;
        use vibravidio_core::{
            CaptureSource, CaptureSourceKind, ColorSpace, ScreenBackend, VideoConfig,
        };
        use vibravidio_ringbuffer::create_pair;
        use vibravidio_thread::Priority;

        use crate::platform::DefaultScreenBackend;

        fn is_wayland_session() -> bool {
            std::env::var("XDG_SESSION_TYPE")
                .map(|v| v.eq_ignore_ascii_case("wayland"))
                .unwrap_or(false)
                || std::env::var("WAYLAND_DISPLAY").is_ok()
        }

        fn has_x11_display() -> bool {
            !is_wayland_session() && std::env::var("DISPLAY").is_ok_and(|v| !v.is_empty())
        }

        fn probe_source() -> CaptureSource {
            CaptureSource {
                id: "x11:root".into(),
                name: "probe".into(),
                kind: CaptureSourceKind::Monitor,
                thumbnail: None,
            }
        }

        async fn open_screen_for_probe() -> bool {
            DefaultScreenBackend::open_source_async(&probe_source())
                .await
                .is_ok()
        }

        async fn open_backend() -> Option<DefaultScreenBackend> {
            DefaultScreenBackend::open_source_async(&probe_source())
                .await
                .ok()
        }

        /// Skip если: Wayland (портал интерактивный) / нет X11 / X11 не открылся.
        async fn should_skip_x11_screen_test() -> bool {
            if is_wayland_session() {
                tracing::debug!(
                    "skipping: Wayland session — screen capture requires \
                     interactive xdg-desktop-portal dialog; use \
                     `cargo test -- --ignored --test-threads=1 wayland_screen_capture_manual`"
                );
                return true;
            }
            if !has_x11_display() {
                tracing::debug!("skipping: no DISPLAY");
                return true;
            }
            if !open_screen_for_probe().await {
                tracing::debug!("skipping: X11 screen not available");
                return true;
            }
            false
        }

        #[tokio::test]
        async fn open_source_async_fails_without_backend() {
            if is_wayland_session() {
                tracing::debug!("skipping: Wayland portal is interactive");
                return;
            }
            if has_x11_display() && open_screen_for_probe().await {
                tracing::debug!("skipping: X11 screen available");
                return;
            }

            let r = DefaultScreenBackend::open_source_async(&probe_source()).await;
            assert!(r.is_err());
        }

        #[tokio::test]
        async fn full_pipeline_receives_frames() {
            if should_skip_x11_screen_test().await {
                return;
            }
            let backend = open_backend().await.expect("backend");

            let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
            let mut screen = Screen::new(config).unwrap();
            let (writer, reader) = create_pair::<8, OwnedFrame>();

            screen
                .run_with(backend, writer, Priority::Normal)
                .expect("run_with");

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

        #[tokio::test]
        async fn format_is_always_bgra() {
            if should_skip_x11_screen_test().await {
                return;
            }
            let backend = open_backend().await.expect("backend");

            let config = VideoConfig::new(640, 480, 60, PixelFormat::Yuyv);
            let mut screen = Screen::new(config).unwrap();
            let (writer, reader) = create_pair::<4, OwnedFrame>();

            screen
                .run_with(backend, writer, Priority::Normal)
                .expect("run_with");

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

        #[tokio::test]
        async fn stop_terminates_producer() {
            if should_skip_x11_screen_test().await {
                return;
            }
            let backend = open_backend().await.expect("backend");

            let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
            let mut screen = Screen::new(config).unwrap();
            let (writer, reader) = create_pair::<4, OwnedFrame>();

            screen
                .run_with(backend, writer, Priority::Normal)
                .expect("run_with");

            std::thread::sleep(Duration::from_millis(200));
            screen.stop();

            while reader.read_frame().is_some() {}

            std::thread::sleep(Duration::from_millis(200));
            assert!(reader.read_frame().is_none());
        }

        #[tokio::test]
        async fn ring_overflow_drops_frame() {
            if should_skip_x11_screen_test().await {
                return;
            }
            let backend = open_backend().await.expect("backend");

            let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
            let mut screen = Screen::new(config).unwrap();
            let (writer, reader) = create_pair::<1, OwnedFrame>();

            screen
                .run_with(backend, writer, Priority::Normal)
                .expect("run_with");

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

        #[tokio::test]
        async fn drop_stops_thread() {
            if should_skip_x11_screen_test().await {
                return;
            }
            let backend = open_backend().await.expect("backend");

            let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
            {
                let mut screen = Screen::new(config).unwrap();
                let (w, _r) = create_pair::<4, OwnedFrame>();
                screen
                    .run_with(backend, w, Priority::Normal)
                    .expect("run_with");
                std::thread::sleep(Duration::from_millis(150));
            }
        }

        #[tokio::test]
        async fn data_size_matches_frame_info() {
            if should_skip_x11_screen_test().await {
                return;
            }
            let backend = open_backend().await.expect("backend");

            let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
            let mut screen = Screen::new(config).unwrap();
            let (writer, reader) = create_pair::<4, OwnedFrame>();

            screen
                .run_with(backend, writer, Priority::Normal)
                .expect("run_with");

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

        #[tokio::test]
        async fn double_run_returns_already_running() {
            if should_skip_x11_screen_test().await {
                return;
            }
            let backend1 = open_backend().await.expect("backend1");
            let backend2 = open_backend().await.expect("backend2");

            let config = VideoConfig::new(640, 480, 60, PixelFormat::Bgra8);
            let mut screen = Screen::new(config).unwrap();
            let (w1, _r1) = create_pair::<4, OwnedFrame>();

            if screen.run_with(backend1, w1, Priority::Normal).is_err() {
                return;
            }

            let (w2, _r2) = create_pair::<4, OwnedFrame>();
            let r = screen.run_with(backend2, w2, Priority::Normal);
            assert!(matches!(r, Err(Error::AlreadyRunning)));

            screen.stop();
        }

        #[tokio::test]
        #[ignore = "requires interactive xdg-desktop-portal dialog; run manually"]
        async fn wayland_screen_capture_manual() {
            let _ = tracing_subscriber::fmt()
                .with_env_filter("debug,vibravidio_screen_wayland=trace")
                .with_test_writer()
                .try_init();

            if !is_wayland_session() {
                tracing::debug!("skipping: not a Wayland session");
                return;
            }

            let backend = DefaultScreenBackend::open_source_async(&probe_source())
                .await
                .expect("open_source_async — должно открыться окно портала");

            let config = VideoConfig::new(1920, 1080, 30, PixelFormat::Bgra8);
            let mut screen = Screen::new(config).unwrap();
            let (writer, reader) = create_pair::<4, OwnedFrame>();

            screen
                .run_with(backend, writer, Priority::Normal)
                .expect("run_with");

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

    // ========================================================================
    // Windows-only tests (stub backend)
    // ========================================================================

    #[cfg(windows)]
    mod windows {
        #[tokio::test]
        async fn open_source_returns_unsupported() {
            use crate::platform::DefaultScreenBackend;
            use vibravidio_core::ScreenBackend;

            let source = vibravidio_core::CaptureSource {
                id: "win:probe".into(),
                name: "probe".into(),
                kind: vibravidio_core::CaptureSourceKind::Monitor,
                thumbnail: None,
            };

            let r = DefaultScreenBackend::open_source(&source);
            assert!(r.is_err(), "stub backend must return Err");
        }
    }
}

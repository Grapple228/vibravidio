use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use vibravidio_core::{CameraBackend, Error, OwnedFrame, Result, VideoBackend, VideoConfig};
use vibravidio_ringbuffer::BufferWriter;
use vibravidio_thread::Priority;
use vibravidio_thread::thread::{self, JoinHandle};

use crate::platform::DefaultCameraBackend;

pub struct Camera {
    config: VideoConfig,
    handle: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    poll_interval: Duration,
}

impl Camera {
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

        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let running_for_setup = running.clone();
        let config = self.config;
        let poll = self.poll_interval;
        let (setup_tx, setup_rx) = std::sync::mpsc::channel::<Result<()>>();

        let handle = thread::spawn(move || {
            let _h = vibravidio_thread::configure_thread(
                priority,
                #[cfg(target_os = "windows")]
                mmcss_value,
            );

            // MF `IMFSourceReader` thread-affine: open/configure ДОЛЖНЫ
            // происходить на том же потоке, где будет capture_frame.
            let mut backend = match DefaultCameraBackend::open("default") {
                Ok(b) => b,
                Err(e) => {
                    let _ = setup_tx.send(Err(e));
                    running_for_setup.store(false, Ordering::SeqCst);
                    return;
                }
            };
            if let Err(e) = backend.configure(&config) {
                let _ = setup_tx.send(Err(e));
                running_for_setup.store(false, Ordering::SeqCst);
                return;
            }
            let _ = setup_tx.send(Ok(()));

            while running.load(Ordering::SeqCst) {
                match backend.capture_frame() {
                    Ok(Some(captured)) => {
                        let frame = OwnedFrame::from_captured(&captured);
                        drop(captured);
                        if writer.write_frame(frame).is_err() {
                            tracing::warn!("video ring full, dropping frame");
                        }
                    }
                    Ok(None) => thread::sleep(poll),
                    Err(Error::SizeChanged { old, new }) => {
                        tracing::warn!("size {old:?} -> {new:?}");
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("❌ Capture error: {e}");
                        break;
                    }
                }
            }
        });

        match setup_rx.recv() {
            Ok(Ok(())) => {
                self.handle = Some(handle);
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                self.running.store(false, Ordering::SeqCst);
                Err(Error::Ffi {
                    code: -1,
                    message: "camera setup thread panicked",
                })
            }
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;
    use vibravidio_core::{CameraBackend, PixelFormat, VideoConfig};
    use vibravidio_ringbuffer::create_pair;
    use vibravidio_thread::Priority;

    static CAMERA_LOCK: Mutex<()> = Mutex::new(());

    fn camera_guard() -> std::sync::MutexGuard<'static, ()> {
        CAMERA_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn any_video_node_exists() -> bool {
        (0..32u32).any(|i| std::path::Path::new(&format!("/dev/video{i}")).exists())
    }

    fn open_camera_for_probe() -> bool {
        DefaultCameraBackend::open("default").is_ok()
    }

    #[test]
    fn camera_new_does_not_open_device() {
        let config = VideoConfig::new(640, 480, 30, PixelFormat::Yuyv);
        let camera = Camera::new(config);
        assert!(camera.is_ok());
    }

    #[test]
    fn run_fails_without_device() {
        let _guard = camera_guard();
        if any_video_node_exists() && open_camera_for_probe() {
            tracing::debug!("skipping: camera is available");
            return;
        }
        let config = VideoConfig::new(640, 480, 30, PixelFormat::Yuyv);
        let mut camera = Camera::new(config).unwrap();
        let (w, _r) = create_pair::<4, OwnedFrame>();
        let r = camera.run(
            w,
            Priority::Normal,
            #[cfg(target_os = "windows")]
            vibravidio_thread::MmcssValue::Video,
        );
        assert!(r.is_err());
    }

    #[test]
    fn full_pipeline_receives_frames() {
        let _guard = camera_guard();
        if !open_camera_for_probe() {
            tracing::debug!("skipping: no camera");
            return;
        }

        let config = VideoConfig::new(640, 480, 30, PixelFormat::Yuyv);
        let mut camera = Camera::new(config).unwrap();
        let (writer, reader) = create_pair::<8, OwnedFrame>();

        camera
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run");

        let start = Instant::now();
        let mut frames: Vec<OwnedFrame> = Vec::new();
        while start.elapsed() < Duration::from_secs(2) && frames.len() < 10 {
            if let Some(f) = reader.read_frame() {
                frames.push(f);
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        camera.stop();

        assert!(!frames.is_empty(), "must receive frames");

        for (i, f) in frames.iter().enumerate() {
            assert!(f.info.width > 0);
            assert!(f.info.height > 0);
            assert!(f.info.stride >= f.info.width * 2);
            assert!(!f.data.is_empty());
            assert_eq!(f.info.format, PixelFormat::Yuyv);
            if i > 0 {
                assert!(f.info.sequence > frames[i - 1].info.sequence);
            }
        }
    }

    #[test]
    fn stop_terminates_producer() {
        let _guard = camera_guard();
        if !open_camera_for_probe() {
            tracing::debug!("skipping: no camera");
            return;
        }

        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        let mut camera = Camera::new(config).unwrap();
        let (writer, reader) = create_pair::<4, OwnedFrame>();

        camera
            .run(
                writer,
                Priority::Normal,
                #[cfg(target_os = "windows")]
                vibravidio_thread::MmcssValue::Video,
            )
            .expect("run");

        std::thread::sleep(Duration::from_millis(300));
        camera.stop();

        while reader.read_frame().is_some() {}

        std::thread::sleep(Duration::from_millis(200));
        assert!(reader.read_frame().is_none());
    }

    #[test]
    fn ring_overflow_drops_frame() {
        let _guard = camera_guard();
        if !open_camera_for_probe() {
            tracing::debug!("skipping: no camera");
            return;
        }

        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        let mut camera = Camera::new(config).unwrap();
        let (writer, reader) = create_pair::<1, OwnedFrame>();

        camera
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
        camera.stop();

        assert!(count >= 1, "must read at least one frame even with N=1");
    }

    #[test]
    fn drop_stops_thread() {
        let _guard = camera_guard();
        if !open_camera_for_probe() {
            tracing::debug!("skipping: no camera");
            return;
        }
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        {
            let mut camera = Camera::new(config).unwrap();
            let (w, _r) = create_pair::<4, OwnedFrame>();
            camera
                .run(
                    w,
                    Priority::Normal,
                    #[cfg(target_os = "windows")]
                    vibravidio_thread::MmcssValue::Video,
                )
                .expect("run");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    #[test]
    fn data_size_matches_configured() {
        let _guard = camera_guard();
        if !open_camera_for_probe() {
            tracing::debug!("skipping: no camera");
            return;
        }

        let config = VideoConfig::new(640, 480, 30, PixelFormat::Yuyv);
        let mut camera = Camera::new(config).unwrap();
        let (writer, reader) = create_pair::<4, OwnedFrame>();

        camera
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
        camera.stop();

        let f = frame.expect("must receive frame");
        assert_eq!(f.info.format, PixelFormat::Yuyv);
        assert!(f.data.len() >= (f.info.stride * f.info.height) as usize);
    }

    #[test]
    fn double_run_returns_already_running() {
        let _guard = camera_guard();
        if !open_camera_for_probe() {
            tracing::debug!("skipping: no camera");
            return;
        }

        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        let mut camera = Camera::new(config).unwrap();
        let (w1, _r1) = create_pair::<4, OwnedFrame>();

        if camera
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
        let r = camera.run(
            w2,
            Priority::Normal,
            #[cfg(target_os = "windows")]
            vibravidio_thread::MmcssValue::Video,
        );
        assert!(matches!(r, Err(Error::AlreadyRunning)));

        camera.stop();
    }
}

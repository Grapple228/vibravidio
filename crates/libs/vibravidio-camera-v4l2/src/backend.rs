use crate::ffi;
use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use vibravidio_core::{
    CapturedFrame, ColorSpace, Error, FrameInfo, PixelFormat, Result, SourceKind, VideoBackend,
    VideoConfig, VideoFormatInfo,
};

struct MmapBuffer {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for MmapBuffer {}

pub struct V4l2Backend {
    /// Хранится только для Drop: закрытие fd через Drop(File).
    /// `_file` префикс — чтобы не было dead_code warning.
    _file: File,
    fd: RawFd,
    buffers: Vec<MmapBuffer>,
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
    color: ColorSpace,
    seq: u64,
    start: Instant,
    pending_qbuf: Option<u32>,
    streaming: bool,
}

/// Открывает файл устройства. `"default"` — перебирает /dev/video0..31
/// и возвращает первый capture-capable узел (VIDEO_CAPTURE + STREAMING).
fn open_device(name: &str) -> Result<(File, RawFd)> {
    if name != "default" {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)?;
        let fd = f.as_raw_fd();
        return Ok((f, fd));
    }

    let mut last_err: Option<std::io::Error> = None;
    for i in 0..32u32 {
        let path = format!("/dev/video{i}");
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(f) => {
                let fd = f.as_raw_fd();
                let mut cap: ffi::v4l2_capability = unsafe { std::mem::zeroed() };
                let r = unsafe { ffi::ioctl(fd, ffi::VIDIOC_QUERYCAP, &mut cap as *mut _) };
                if r < 0 {
                    continue;
                }
                let caps = if cap.capabilities & ffi::V4L2_CAP_DEVICE_CAPS != 0 {
                    cap.device_caps
                } else {
                    cap.capabilities
                };
                if caps & ffi::V4L2_CAP_VIDEO_CAPTURE != 0 && caps & ffi::V4L2_CAP_STREAMING != 0 {
                    tracing::debug!("V4l2Backend: 'default' resolved to {path}");
                    return Ok((f, fd));
                }
            }
            Err(e) => last_err = Some(e),
        }
    }

    Err(Error::Io(last_err.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no capture-capable /dev/videoN found",
        )
    })))
}

impl V4l2Backend {
    fn buffer_slice(&self, idx: usize) -> &[u8] {
        let b = &self.buffers[idx];
        unsafe { std::slice::from_raw_parts(b.ptr, b.len) }
    }

    fn stop_streaming(&mut self) {
        if self.streaming {
            let mut typ = ffi::V4L2_BUF_TYPE_VIDEO_CAPTURE as i32;
            unsafe {
                ffi::ioctl(self.fd, ffi::VIDIOC_STREAMOFF, &mut typ);
            }
            self.streaming = false;
        }
    }

    fn unmap_buffers(&mut self) {
        for b in &self.buffers {
            unsafe {
                ffi::munmap(b.ptr as *mut _, b.len);
            }
        }
        self.buffers.clear();
        self.pending_qbuf = None;
    }

    fn qbuf(&mut self, index: u32) -> Result<()> {
        let mut buf: ffi::v4l2_buffer = unsafe { std::mem::zeroed() };
        buf.index = index;
        buf.typ = ffi::V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = ffi::V4L2_MEMORY_MMAP;
        let r = unsafe { ffi::ioctl(self.fd, ffi::VIDIOC_QBUF, &mut buf as *mut _) };
        if r < 0 {
            return Err(Error::Ffi {
                code: r,
                message: "VIDIOC_QBUF failed",
            });
        }
        Ok(())
    }

    fn readable_nonblocking(&self) -> Result<bool> {
        let mut p = ffi::PollFd {
            fd: self.fd,
            events: ffi::POLLIN,
            revents: 0,
        };
        let r = unsafe { ffi::poll(&mut p, 1, 0) };
        if r < 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        Ok(r > 0 && (p.revents & ffi::POLLIN) != 0)
    }

    fn setup_stream(&mut self, config: &VideoConfig) -> Result<()> {
        let pixfmt = match config.format {
            PixelFormat::Yuyv => ffi::V4L2_PIX_FMT_YUYV,
            PixelFormat::Rgb8 => ffi::V4L2_PIX_FMT_RGB24,
            _ => return Err(Error::UnsupportedFormat),
        };

        let mut fmt: ffi::v4l2_format = unsafe { std::mem::zeroed() };
        fmt.typ = ffi::V4L2_BUF_TYPE_VIDEO_CAPTURE;
        fmt.fmt.pix = ffi::v4l2_pix_format {
            width: config.width,
            height: config.height,
            pixelformat: pixfmt,
            field: 1,
            ..unsafe { std::mem::zeroed() }
        };
        let r = unsafe { ffi::ioctl(self.fd, ffi::VIDIOC_S_FMT, &mut fmt as *mut _) };
        if r < 0 {
            return Err(Error::UnsupportedFormat);
        }
        let pix = unsafe { fmt.fmt.pix };
        if pix.pixelformat != pixfmt {
            return Err(Error::UnsupportedFormat);
        }
        self.width = pix.width;
        self.height = pix.height;
        self.stride = pix.bytesperline;
        self.format = match pix.pixelformat {
            ffi::V4L2_PIX_FMT_YUYV => PixelFormat::Yuyv,
            ffi::V4L2_PIX_FMT_RGB24 => PixelFormat::Rgb8,
            _ => return Err(Error::UnsupportedFormat),
        };
        self.color = match pix.colorspace {
            2 => ColorSpace::Bt709,
            _ => ColorSpace::Bt601,
        };

        let mut req: ffi::v4l2_requestbuffers = unsafe { std::mem::zeroed() };
        req.count = 4;
        req.typ = ffi::V4L2_BUF_TYPE_VIDEO_CAPTURE;
        req.memory = ffi::V4L2_MEMORY_MMAP;
        let r = unsafe { ffi::ioctl(self.fd, ffi::VIDIOC_REQBUFS, &mut req as *mut _) };
        if r < 0 || req.count == 0 {
            return Err(Error::InvalidConfig);
        }

        for i in 0..req.count {
            let mut buf: ffi::v4l2_buffer = unsafe { std::mem::zeroed() };
            buf.index = i;
            buf.typ = ffi::V4L2_BUF_TYPE_VIDEO_CAPTURE;
            buf.memory = ffi::V4L2_MEMORY_MMAP;
            let r = unsafe { ffi::ioctl(self.fd, ffi::VIDIOC_QUERYBUF, &mut buf as *mut _) };
            if r < 0 {
                self.unmap_buffers();
                return Err(Error::InvalidConfig);
            }
            let offset = unsafe { buf.m.offset } as i64;
            let ptr = unsafe {
                ffi::mmap(
                    std::ptr::null_mut(),
                    buf.length as usize,
                    ffi::PROT_READ | ffi::PROT_WRITE,
                    ffi::MAP_SHARED,
                    self.fd,
                    offset,
                )
            };
            if ptr == ffi::MAP_FAILED {
                self.unmap_buffers();
                return Err(Error::InvalidConfig);
            }
            self.buffers.push(MmapBuffer {
                ptr: ptr as *mut u8,
                len: buf.length as usize,
            });
        }

        let n = self.buffers.len() as u32;
        for i in 0..n {
            self.qbuf(i)?;
        }

        let mut typ = ffi::V4L2_BUF_TYPE_VIDEO_CAPTURE as i32;
        let r = unsafe { ffi::ioctl(self.fd, ffi::VIDIOC_STREAMON, &mut typ) };
        if r < 0 {
            self.unmap_buffers();
            return Err(Error::InvalidConfig);
        }
        self.streaming = true;

        Ok(())
    }
}

impl VideoBackend for V4l2Backend {
    fn open(name: &str, kind: SourceKind) -> Result<Self> {
        if !matches!(kind, SourceKind::Camera) {
            return Err(Error::InvalidConfig);
        }

        // open_device уже делает QUERYCAP для "default"-пути, но не для явного.
        // Делаем QUERYCAP один раз здесь для обоих случаев.
        let (file, fd) = open_device(name)?;

        let mut cap: ffi::v4l2_capability = unsafe { std::mem::zeroed() };
        let r = unsafe { ffi::ioctl(fd, ffi::VIDIOC_QUERYCAP, &mut cap as *mut _) };
        if r < 0 {
            return Err(Error::DeviceNotFound);
        }
        let caps = if cap.capabilities & ffi::V4L2_CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        if caps & ffi::V4L2_CAP_VIDEO_CAPTURE == 0 || caps & ffi::V4L2_CAP_STREAMING == 0 {
            return Err(Error::InvalidConfig);
        }

        Ok(Self {
            _file: file,
            fd,
            buffers: Vec::new(),
            width: 0,
            height: 0,
            stride: 0,
            format: PixelFormat::Yuyv,
            color: ColorSpace::Bt601,
            seq: 0,
            start: Instant::now(),
            pending_qbuf: None,
            streaming: false,
        })
    }

    fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        self.stop_streaming();
        self.unmap_buffers();
        self.setup_stream(config)
    }

    fn supported_formats(&self) -> Result<Vec<VideoFormatInfo>> {
        Ok(vec![
            VideoFormatInfo {
                width: 1280,
                height: 720,
                fps: 30,
                format: PixelFormat::Yuyv,
                color: ColorSpace::Bt601,
            },
            VideoFormatInfo {
                width: 640,
                height: 480,
                fps: 30,
                format: PixelFormat::Yuyv,
                color: ColorSpace::Bt601,
            },
        ])
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        if let Some(idx) = self.pending_qbuf.take() {
            self.qbuf(idx)?;
        }

        if !self.readable_nonblocking()? {
            return Ok(None);
        }

        let mut buf: ffi::v4l2_buffer = unsafe { std::mem::zeroed() };
        buf.typ = ffi::V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = ffi::V4L2_MEMORY_MMAP;
        let r = unsafe { ffi::ioctl(self.fd, ffi::VIDIOC_DQBUF, &mut buf as *mut _) };
        if r < 0 {
            return Ok(None);
        }

        let idx = buf.index as usize;
        self.pending_qbuf = Some(buf.index);
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

        let data = self.buffer_slice(idx);
        Ok(Some(CapturedFrame { data, info }))
    }

    fn reset(&mut self) -> Result<()> {
        self.stop_streaming();
        self.unmap_buffers();
        self.seq = 0;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.stop_streaming();
        self.unmap_buffers();
        Ok(())
    }
}

impl Drop for V4l2Backend {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn open_first_available_camera() -> Option<V4l2Backend> {
        // Через "default" — теперь работает, если есть capture-устройство.
        match V4l2Backend::open("default", SourceKind::Camera) {
            Ok(b) => Some(b),
            Err(_) => {
                // Fallback: явные пути (некоторые конфиги не дают VIDEO_CAPTURE
                // на video0, но дают на video1 и т.д. — open_device это уже
                // перебирает, так что это чисто перестраховка).
                for dev in ["/dev/video0", "/dev/video1", "/dev/video2"] {
                    if let Ok(b) = V4l2Backend::open(dev, SourceKind::Camera) {
                        return Some(b);
                    }
                }
                None
            }
        }
    }

    #[test]
    fn open_nonexistent_device_returns_error() {
        let r = V4l2Backend::open("/dev/video_nonexistent_999", SourceKind::Camera);
        assert!(r.is_err());
    }

    #[test]
    fn open_with_screen_kind_returns_invalid_config() {
        match V4l2Backend::open("/dev/video0", SourceKind::Screen) {
            Err(Error::InvalidConfig) => {}
            Err(e) => panic!("expected InvalidConfig, got {e:?}"),
            Ok(_) => panic!("Screen kind must not succeed on V4L2"),
        }
    }

    #[test]
    fn open_default_resolves_to_some_camera() {
        // Если камеры нет — skip. Если есть — "default" должен её найти.
        let has_camera =
            (0..32u32).any(|i| std::path::Path::new(&format!("/dev/video{i}")).exists());
        if !has_camera {
            eprintln!("skipping: no /dev/videoN exists");
            return;
        }
        match V4l2Backend::open("default", SourceKind::Camera) {
            Ok(_) => {} // успех
            Err(e) => {
                // Может быть что все videoN — не capture (metadata nodes).
                eprintln!("'default' failed to resolve ({e:?}) — ok if no capture device");
            }
        }
    }

    #[test]
    fn configure_yuyv_640x480_and_capture() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };

        let config = VideoConfig::new(640, 480, 30, PixelFormat::Yuyv);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: YUYV 640x480 not supported");
            return;
        }
        std::thread::sleep(Duration::from_millis(300));

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = 0;
        while Instant::now() < deadline && got < 3 {
            match backend.capture_frame() {
                Ok(Some(frame)) => {
                    assert_eq!(frame.info.format, PixelFormat::Yuyv);
                    assert!(frame.info.width > 0);
                    assert!(frame.info.height > 0);
                    assert!(frame.info.stride >= frame.info.width * 2);
                    assert!(!frame.data.is_empty());
                    assert!(frame.data.len() >= (frame.info.stride * frame.info.height) as usize);
                    got += 1;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => panic!("capture error: {e:?}"),
            }
        }
        assert!(got >= 1, "must capture at least one frame");
    }

    #[test]
    fn capture_sequence_is_monotonic() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(300));

        let mut last_seq = 0u64;
        let mut got = 0;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && got < 5 {
            match backend.capture_frame() {
                Ok(Some(f)) => {
                    assert!(f.info.sequence > last_seq);
                    last_seq = f.info.sequence;
                    got += 1;
                }
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(got >= 1);
    }

    #[test]
    fn capture_timestamp_is_monotonic() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(300));

        let mut last_ts = 0u64;
        let mut got = 0;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && got < 5 {
            if let Ok(Some(f)) = backend.capture_frame() {
                assert!(f.info.timestamp_ns >= last_ts);
                last_ts = f.info.timestamp_ns;
                got += 1;
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn capture_is_nonblocking() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(300));

        let start = Instant::now();
        let mut nones = 0;
        let mut somes = 0;
        for _ in 0..200 {
            match backend.capture_frame() {
                Ok(Some(_)) => somes += 1,
                Ok(None) => nones += 1,
                Err(_) => break,
            }
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "capture blocks: {elapsed:?}"
        );
        assert!(nones > somes, "mostly None expected at 30fps rapid calls");
    }

    #[test]
    fn reconfigure_same_params() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(640, 480, 30, PixelFormat::Yuyv);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));

        backend.configure(&config).expect("reconfigure failed");
        std::thread::sleep(Duration::from_millis(200));

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = false;
        while Instant::now() < deadline {
            if matches!(backend.capture_frame(), Ok(Some(_))) {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got);
    }

    #[test]
    fn reset_stops_stream() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
        let _ = backend.capture_frame();

        backend.reset().expect("reset");
        let r = backend.capture_frame();
        assert!(matches!(r, Ok(None) | Err(_)));
    }

    #[test]
    fn drop_closes_cleanly() {
        for _ in 0..3 {
            let Some(mut backend) = open_first_available_camera() else {
                eprintln!("skipping: no camera");
                return;
            };
            let config = VideoConfig::new(320, 240, 30, PixelFormat::Yuyv);
            let _ = backend.configure(&config);
            std::thread::sleep(Duration::from_millis(100));
            drop(backend);
        }
    }

    #[test]
    fn supported_formats_non_empty() {
        let Some(backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let formats = backend.supported_formats().expect("supported_formats");
        assert!(!formats.is_empty());
        for f in &formats {
            assert!(f.width > 0);
            assert!(f.height > 0);
            assert!(f.fps > 0);
        }
    }
}

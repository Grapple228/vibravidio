use std::ptr;
use std::time::Instant;

use tracing::info;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::core::PWSTR;

use vibravidio_core::{
    CameraBackend, CapturedFrame, ColorSpace, Error, FrameInfo, PixelFormat, Result, VideoBackend,
    VideoConfig, VideoFormatInfo,
};

pub struct MfCameraBackend {
    reader: IMFSourceReader,
    buffer: Vec<u8>,
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
    color: ColorSpace,
    seq: u64,
    start: Instant,
}

// COM инициализирован в MTA, IMFSourceReader не привязан к STA-потоку —
// передача между потоками безопасна.
unsafe impl Send for MfCameraBackend {}

fn ff(code: i32, message: &'static str) -> Error {
    Error::Ffi { code, message }
}

/// Idempotent per-thread COM init (MTA). Required for MF on every
/// thread that touches COM objects.
fn ensure_com() {
    thread_local! {
        static INIT: () = unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); };
    }
    INIT.with(|_| {});
}

impl CameraBackend for MfCameraBackend {
    fn open(name: &str) -> Result<Self> {
        ensure_com();

        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| ff(e.code().0, "MFStartup"))?;
        }

        let devices = enumerate_devices()?;
        let activate = select_device(&devices, name)?;

        let reader = create_reader(&activate)?;

        // Начальный формат — NV12 в native-разрешении камеры. Конкретный
        // размер/формат задаст `configure` перед запуском.
        let (w, h, s, f, c) =
            set_media_type(&reader, &VideoConfig::new(0, 0, 30, PixelFormat::Nv12))?;

        info!("✅ Camera opened: {}x{} {:?}", w, h, f);

        Ok(Self {
            reader,
            buffer: Vec::new(),
            width: w,
            height: h,
            stride: s,
            format: f,
            color: c,
            seq: 0,
            start: Instant::now(),
        })
    }

    fn supported_formats(&self) -> Result<Vec<VideoFormatInfo>> {
        // Перебираем native media types через `GetNativeMediaType` и
        // собираем уникальные (width, height, fps, subtype).
        unsafe {
            let mut out: Vec<VideoFormatInfo> = Vec::new();
            for idx in 0u32..32 {
                let mt = match self
                    .reader
                    .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, idx)
                {
                    Ok(mt) => mt,
                    Err(_) => break,
                };

                let Ok(frame_size) = mt.GetUINT64(&MF_MT_FRAME_SIZE) else {
                    continue;
                };
                let width = (frame_size >> 32) as u32;
                let height = (frame_size & 0xFFFF_FFFF) as u32;
                if width == 0 || height == 0 {
                    continue;
                }

                let fps = match mt.GetUINT64(&MF_MT_FRAME_RATE) {
                    Ok(v) => {
                        let num = (v >> 32) as u32;
                        let den = (v & 0xFFFF_FFFF) as u32;
                        if den > 0 { num / den } else { num }
                    }
                    Err(_) => 0,
                };

                let Ok(subtype) = mt.GetGUID(&MF_MT_SUBTYPE) else {
                    continue;
                };
                let format = if subtype == MFVideoFormat_NV12 {
                    PixelFormat::Nv12
                } else if subtype == MFVideoFormat_YUY2 {
                    PixelFormat::Yuyv
                } else if subtype == MFVideoFormat_RGB32 {
                    PixelFormat::Bgra8
                } else {
                    // Пропускаем MJPEG и прочие — VideoProcessor сконвертирует,
                    // но как «нативный формат» камеры это не то, что мы отдаём.
                    continue;
                };

                let info = VideoFormatInfo {
                    width,
                    height,
                    fps: fps as usize,
                    format,
                    color: ColorSpace::Bt709,
                };
                if !out.contains(&info) {
                    out.push(info);
                }
            }

            out.sort_by(|a, b| {
                a.width
                    .cmp(&b.width)
                    .then(a.height.cmp(&b.height))
                    .then(a.fps.cmp(&b.fps))
            });
            Ok(out)
        }
    }
}

impl VideoBackend for MfCameraBackend {
    fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        ensure_com();
        let (w, h, s, f, c) = set_media_type(&self.reader, config)?;
        self.width = w;
        self.height = h;
        self.stride = s;
        self.format = f;
        self.color = c;
        Ok(())
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        ensure_com();

        unsafe {
            let mut sample: Option<IMFSample> = None;
            let mut flags: u32 = 0;
            self.reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
                .map_err(|e| ff(e.code().0, "ReadSample"))?;

            if flags as i32 & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                return Ok(None);
            }

            let Some(sample) = sample else {
                return Ok(None);
            };

            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| ff(e.code().0, "ConvertToContiguousBuffer"))?;

            let mut ptr: *mut u8 = ptr::null_mut();
            let mut max_len: u32 = 0;
            let mut cur_len: u32 = 0;
            buffer
                .Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
                .map_err(|e| ff(e.code().0, "Lock"))?;

            self.buffer.clear();
            self.buffer
                .extend_from_slice(std::slice::from_raw_parts(ptr, cur_len as usize));

            buffer.Unlock().map_err(|e| ff(e.code().0, "Unlock"))?;

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
            Ok(Some(CapturedFrame {
                data: &self.buffer,
                info,
            }))
        }
    }

    fn reset(&mut self) -> Result<()> {
        unsafe {
            self.reader
                .Flush(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|e| ff(e.code().0, "Flush"))?;
        }
        self.seq = 0;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        // `IMFSourceReader` releases its COM pointer on Drop. Не зовём
        // `MFShutdown` — runtime живёт до process exit, чтобы новые
        // `Camera::new()` работали.
        Ok(())
    }
}

// ============================================================================
// Device enumeration
// ============================================================================

fn enumerate_devices() -> Result<Vec<(String, IMFActivate)>> {
    unsafe {
        let mut attrs_opt: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs_opt, 1).map_err(|e| ff(e.code().0, "MFCreateAttributes"))?;
        let attrs = attrs_opt.ok_or(ff(-1, "MFCreateAttributes returned None"))?;

        attrs
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(|e| ff(e.code().0, "SetGUID source_type"))?;

        let mut devices_ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count: u32 = 0;

        let enum_result = MFEnumDeviceSources(&attrs, &mut devices_ptr, &mut count);
        if enum_result.is_err() || devices_ptr.is_null() || count == 0 {
            if !devices_ptr.is_null() {
                CoTaskMemFree(Some(devices_ptr as *const _));
            }
            return Err(Error::DeviceNotFound);
        }

        let mut result = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let Some(activate) = (*devices_ptr.add(i)).take() else {
                continue;
            };

            let mut name_ptr: PWSTR = PWSTR::null();
            let mut name_len: u32 = 0;
            let name = if activate
                .GetAllocatedString(
                    &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                    &mut name_ptr,
                    &mut name_len,
                )
                .is_ok()
                && !name_ptr.is_null()
            {
                let s = name_ptr
                    .to_string()
                    .unwrap_or_else(|_| format!("Camera {}", i + 1));
                CoTaskMemFree(Some(name_ptr.0 as *const _));
                s
            } else {
                format!("Camera {}", i + 1)
            };

            result.push((name, activate));
        }

        CoTaskMemFree(Some(devices_ptr as *const _));
        Ok(result)
    }
}

fn select_device(devices: &[(String, IMFActivate)], name: &str) -> Result<IMFActivate> {
    if devices.is_empty() {
        return Err(Error::DeviceNotFound);
    }
    if name == "default" || name.is_empty() {
        return Ok(devices[0].1.clone());
    }
    let needle = name.to_lowercase();
    devices
        .iter()
        .find(|(n, _)| n.to_lowercase().contains(&needle))
        .map(|(_, a)| a.clone())
        .ok_or(Error::DeviceNotFound)
}

// ============================================================================
// SourceReader
// ============================================================================

fn create_reader(activate: &IMFActivate) -> Result<IMFSourceReader> {
    unsafe {
        let source: IMFMediaSource = activate
            .ActivateObject()
            .map_err(|e| ff(e.code().0, "ActivateObject"))?;

        let mut attrs_opt: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs_opt, 2).map_err(|e| ff(e.code().0, "MFCreateAttributes"))?;
        let attrs = attrs_opt.ok_or(ff(-1, "MFCreateAttributes returned None"))?;

        // Авто-конверсия YUY2/MJPEG → NV12 через MF Video Processor
        // (использует GPU, когда возможно). Best-effort — игнорируем
        // ошибки, некоторые системы не имеют VideoProcessor MFT.
        let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);
        let _ = attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1);

        MFCreateSourceReaderFromMediaSource(&source, Some(&attrs))
            .map_err(|e| ff(e.code().0, "MFCreateSourceReaderFromMediaSource"))
    }
}

fn set_media_type(
    reader: &IMFSourceReader,
    config: &VideoConfig,
) -> Result<(u32, u32, u32, PixelFormat, ColorSpace)> {
    unsafe {
        let media_type = MFCreateMediaType().map_err(|e| ff(e.code().0, "MFCreateMediaType"))?;

        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| ff(e.code().0, "SetGUID major"))?;

        // NV12 — самый универсальный. VideoProcessing сконвертирует
        // YUY2/MJPEG, если камера нативно NV12 не отдаёт.
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| ff(e.code().0, "SetGUID NV12"))?;

        let _ = media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);

        if config.width > 0 && config.height > 0 {
            let frame_size = ((config.width as u64) << 32) | (config.height as u64);
            let _ = media_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size);
        }

        if config.fps > 0 {
            let frame_rate = ((config.fps as u64) << 32) | 1u64;
            let _ = media_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate);
        }

        reader
            .SetCurrentMediaType(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                None,
                &media_type,
            )
            .map_err(|e| ff(e.code().0, "SetCurrentMediaType NV12"))?;

        // Читаем что фактически согласовалось (может отличаться от запрошенного).
        let actual = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            .map_err(|e| ff(e.code().0, "GetCurrentMediaType"))?;

        let frame_size = actual
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .map_err(|e| ff(e.code().0, "GetUINT64 frame_size"))?;
        let width = (frame_size >> 32) as u32;
        let height = (frame_size & 0xFFFF_FFFF) as u32;

        let subtype = actual
            .GetGUID(&MF_MT_SUBTYPE)
            .map_err(|e| ff(e.code().0, "GetGUID subtype"))?;

        let (format, stride) = if subtype == MFVideoFormat_NV12 {
            (PixelFormat::Nv12, width)
        } else if subtype == MFVideoFormat_YUY2 {
            (PixelFormat::Yuyv, width * 2)
        } else if subtype == MFVideoFormat_RGB32 {
            (PixelFormat::Bgra8, width * 4)
        } else {
            tracing::warn!("unexpected MF subtype {:?}, assuming NV12", subtype);
            (PixelFormat::Nv12, width)
        };

        Ok((width, height, stride, format, ColorSpace::Bt709))
    }
}

// ============================================================================
// Tests
// ============================================================================
//
// Поведение Windows-бэкенда отличается от V4L2:
//   * `ReadSample` синхронный — блокируется до прихода кадра (нет poll()-а).
//   * `reset()` делает Flush, а не STREAMOFF — после него capture продолжает
//     работать, обнуляется только счётчик `seq`.
//   * Нет прямого «нет камеры» состояния в open — enumerate вернёт
//     DeviceNotFound, что мы и проверяем.
//
// Поэтому тесты адаптированы: там, где у Linux был строгий non-blocking
// контракт, у Windows проверяется ограниченное время выполнения. Там, где
// у Linux reset → стрим остановлен, у Windows reset → seq == 0.
//
// Все тесты, требующие реальную камеру, «skip» (eprintln + return),
// если её нет — CI без железа не краснеет.

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Пытается открыть "default". `None` — камеры нет (skip теста).
    fn open_first_available_camera() -> Option<MfCameraBackend> {
        MfCameraBackend::open("default").ok()
    }

    #[test]
    fn backend_implements_traits() {
        fn _assert_camera<B: CameraBackend>() {}
        fn _assert_video<B: VideoBackend>() {}
        _assert_camera::<MfCameraBackend>();
        _assert_video::<MfCameraBackend>();
    }

    #[test]
    fn open_unknown_name_returns_error() {
        // "nonexistent_xyz" не совпадёт ни с одной камерой по имени.
        // На системе без камер — DeviceNotFound на этапе enumerate.
        // На системе с камерой — DeviceNotFound на этапе select.
        let r = MfCameraBackend::open("nonexistent_camera_name_xyz_9999");
        assert!(r.is_err());
    }

    #[test]
    fn open_default_resolves_to_some_camera() {
        // Если камеры нет — skip. Если есть — "default" должен её открыть.
        match MfCameraBackend::open("default") {
            Ok(_) => {} // успех
            Err(Error::DeviceNotFound) => {
                eprintln!("skipping: no camera available");
            }
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }

    #[test]
    fn configure_nv12_640x480_and_capture() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };

        let config = VideoConfig::new(640, 480, 30, PixelFormat::Nv12);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: NV12 640x480 not supported");
            return;
        }
        // Дать камере время на прогрев после SetCurrentMediaType.
        std::thread::sleep(Duration::from_millis(300));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = 0;
        while Instant::now() < deadline && got < 3 {
            match backend.capture_frame() {
                Ok(Some(frame)) => {
                    assert!(frame.info.width > 0);
                    assert!(frame.info.height > 0);
                    // NV12 stride = width; YUY2 — width * 2; BGRA8 — width * 4.
                    assert!(frame.info.stride >= frame.info.width);
                    assert!(!frame.data.is_empty());
                    assert!(
                        frame.data.len() >= (frame.info.stride * frame.info.height) as usize,
                        "buffer too small for NV12: {} < stride*height={}",
                        frame.data.len(),
                        frame.info.stride * frame.info.height
                    );
                    got += 1;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
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
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Nv12);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(300));

        let mut last_seq = 0u64;
        let mut got = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && got < 5 {
            match backend.capture_frame() {
                Ok(Some(f)) => {
                    assert!(f.info.sequence > last_seq);
                    last_seq = f.info.sequence;
                    got += 1;
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
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
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Nv12);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(300));

        let mut last_ts = 0u64;
        let mut got = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && got < 5 {
            if let Ok(Some(f)) = backend.capture_frame() {
                assert!(f.info.timestamp_ns >= last_ts);
                last_ts = f.info.timestamp_ns;
                got += 1;
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    #[test]
    fn capture_completes_in_bounded_time() {
        // Windows MF `ReadSample` синхронный (блокируется до кадра).
        // Контракт "non-blocking" из V4L2 здесь невыполним без async
        // callbacks; вместо этого проверяем, что вызов не «зависает»
        // дольше разумного таймаута (2s на кадр при 30 fps — с запасом).
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Nv12);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(300));

        for _ in 0..3 {
            let t0 = Instant::now();
            let _ = backend.capture_frame();
            let elapsed = t0.elapsed();
            assert!(
                elapsed < Duration::from_secs(2),
                "capture_frame took too long: {elapsed:?}"
            );
        }
    }

    #[test]
    fn reconfigure_same_params() {
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(640, 480, 30, PixelFormat::Nv12);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));

        backend.configure(&config).expect("reconfigure failed");
        std::thread::sleep(Duration::from_millis(200));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = false;
        while Instant::now() < deadline {
            if matches!(backend.capture_frame(), Ok(Some(_))) {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(got);
    }

    #[test]
    fn reset_clears_sequence() {
        // На Windows `reset()` = Flush + seq=0, а не остановка стрима
        // (в отличие от V4L2 STREAMOFF). Проверяем наблюдаемый контракт:
        // после reset счётчик обнулён, а capture продолжает работать.
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(320, 240, 30, PixelFormat::Nv12);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));

        // Дождаться хотя бы одного кадра, чтобы seq стал > 0.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut captured = false;
        while Instant::now() < deadline {
            if matches!(backend.capture_frame(), Ok(Some(_))) {
                captured = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !captured {
            eprintln!("skipping: no frame captured");
            return;
        }
        assert!(backend.seq > 0);

        backend.reset().expect("reset");
        assert_eq!(backend.seq, 0, "reset must clear sequence counter");
    }

    #[test]
    fn drop_closes_cleanly() {
        for _ in 0..3 {
            let Some(mut backend) = open_first_available_camera() else {
                eprintln!("skipping: no camera");
                return;
            };
            let config = VideoConfig::new(320, 240, 30, PixelFormat::Nv12);
            let _ = backend.configure(&config);
            std::thread::sleep(Duration::from_millis(100));
            drop(backend);
        }
    }

    #[test]
    fn frame_metadata_matches_backend_state() {
        // FrameInfo, отдаваемый в CapturedFrame, должен согласовываться
        // с тем, что backend фактически согласовал с MF (не с запрошенным
        // в config — MF может отдать другой размер).
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(640, 480, 30, PixelFormat::Nv12);
        if backend.configure(&config).is_err() {
            eprintln!("skipping: config unsupported");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut captured: Option<FrameInfo> = None;
        while Instant::now() < deadline && captured.is_none() {
            match backend.capture_frame() {
                Ok(Some(frame)) => captured = Some(frame.info),
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => panic!("capture error: {e:?}"),
            }
        }
        let info = captured.expect("no frame captured within deadline");

        assert_eq!(info.width, backend.width);
        assert_eq!(info.height, backend.height);
        assert_eq!(info.stride, backend.stride);
        assert_eq!(info.format, backend.format);
        assert_eq!(info.color, backend.color);
    }

    #[test]
    fn supported_formats_returns_ok() {
        // `supported_formats` реализован через GetNativeMediaType.
        // На камере, отдающей только MJPEG, список может быть пустым
        // (мы не возвращаем MJPEG как «нативный» формат) — это OK:
        // VideoProcessor всё равно сконвертирует в NV12.
        let Some(backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let formats = backend
            .supported_formats()
            .expect("supported_formats must not error");
        eprintln!("native formats: {} entries", formats.len());
        for f in &formats {
            assert!(f.width > 0);
            assert!(f.height > 0);
            // fps может быть 0, если камера не сообщает MF_MT_FRAME_RATE.
        }
    }
}

#![allow(non_snake_case)]

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Media::DirectShow::{
    IAMStreamConfig, IBaseFilter, ICaptureGraphBuilder2, ICreateDevEnum, IGraphBuilder,
    IMediaControl, IPin, PINDIR_OUTPUT,
};
use windows::Win32::Media::MediaFoundation::AM_MEDIA_TYPE;
use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    IBindCtx, IEnumMoniker, IMoniker,
};
use windows::Win32::System::Variant::{VARIANT, VariantToStringAlloc};
use windows::core::{BOOL, GUID, Interface};

use vibravidio_core::{
    CameraBackend, CapturedFrame, ColorSpace, Error, FrameInfo, PixelFormat, Result, VideoBackend,
    VideoConfig, VideoFormatInfo,
};

const CLSID_FILTER_GRAPH: GUID = GUID::from_u128(0xE436EBB3_524F_11CE_9F53_0020AF0BA770);
const CLSID_SYSTEM_DEVICE_ENUM: GUID = GUID::from_u128(0x62BE5D10_60EB_11D0_BD3B_00A0C911CE86);
const CLSID_VIDEO_INPUT_DEVICE_CATEGORY: GUID =
    GUID::from_u128(0x860BB310_5D01_11D0_BD3B_00A0C911CE86);
const CLSID_SAMPLE_GRABBER: GUID = GUID::from_u128(0xC1F400A0_3F08_11D3_9F0B_006008039E37);
const CLSID_NULL_RENDERER: GUID = GUID::from_u128(0xC1F400A4_3F08_11D3_9F0B_006008039E37);
const CLSID_CAPTURE_GRAPH_BUILDER2: GUID = GUID::from_u128(0xBF87B6E1_8C27_11D0_B3F0_00AA003761C5);

/// E_FAIL — DirectShow отдаёт его из RenderStream, когда драйвер камеры ещё
/// не отпустил устройство после предыдущего Stop()+drop графа. Ретраим.
const E_FAIL_CODE: i32 = 0x80004005u32 as i32;

mod ds {
    use windows::Win32::Foundation::RECT;
    use windows::core::GUID;

    pub const MEDIATYPE_VIDEO: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00AA00389B71);
    pub const MEDIASUBTYPE_RGB24: GUID = GUID::from_u128(0xE436EB7D_524F_11CE_9F53_0020AF0BA770);
    pub const MEDIASUBTYPE_RGB32: GUID = GUID::from_u128(0xE436EB7E_524F_11CE_9F53_0020AF0BA770);
    pub const MEDIASUBTYPE_YUY2: GUID = GUID::from_u128(0x32595559_0000_0010_8000_00AA00389B71);
    pub const MEDIASUBTYPE_NV12: GUID = GUID::from_u128(0x3231564E_0000_0010_8000_00AA00389B71);
    pub const FORMAT_VIDEO_INFO: GUID = GUID::from_u128(0x05589F80_C356_11CE_BF01_00AA0055595A);

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct BitmapInfoHeader {
        pub biSize: u32,
        pub biWidth: i32,
        pub biHeight: i32,
        pub biPlanes: u16,
        pub biBitCount: u16,
        pub biCompression: u32,
        pub biSizeImage: u32,
        pub biXPelsPerMeter: i32,
        pub biYPelsPerMeter: i32,
        pub biClrUsed: u32,
        pub biClrImportant: u32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct VideoInfoHeader {
        pub rcSource: RECT,
        pub rcTarget: RECT,
        pub dwBitRate: u32,
        pub dwBitErrorRate: u32,
        pub AvgTimePerFrame: i64,
        pub bmiHeader: BitmapInfoHeader,
    }
}

#[windows_core::interface("6B652FFF-11FE-4FCE-92AD-0266B5D7C78F")]
unsafe trait ISampleGrabber: windows_core::IUnknown {
    unsafe fn SetOneShot(&self, oneshot: BOOL) -> windows_core::Result<()>;
    unsafe fn SetMediaType(&self, pmt: *const AM_MEDIA_TYPE) -> windows_core::Result<()>;
    unsafe fn GetConnectedMediaType(&self, pmt: *mut AM_MEDIA_TYPE) -> windows_core::Result<()>;
    unsafe fn SetBufferSamples(&self, buffer: BOOL) -> windows_core::Result<()>;
    unsafe fn GetCurrentBuffer(
        &self,
        pcbbuf: *mut u32,
        pbuffer: *mut u8,
    ) -> windows_core::Result<()>;
    unsafe fn GetCurrentSample(
        &self,
        pp_sample: *mut *mut core::ffi::c_void,
    ) -> windows_core::Result<()>;
    unsafe fn SetCallback(
        &self,
        callback: *mut core::ffi::c_void,
        method: i32,
    ) -> windows_core::Result<()>;
}

#[windows_core::interface("0579154A-2B53-4994-B0D0-E773148EFF85")]
unsafe trait ISampleGrabberCB: windows_core::IUnknown {
    unsafe fn SampleCB(
        &self,
        sample_time: f64,
        sample: *mut core::ffi::c_void,
    ) -> windows_core::Result<()>;
    unsafe fn BufferCB(
        &self,
        sample_time: f64,
        buffer: *mut u8,
        buffer_len: i32,
    ) -> windows_core::Result<()>;
}

struct GrabberState {
    scratch: Mutex<Vec<u8>>,
    pending: Mutex<Vec<u8>>,
    seq: AtomicU64,

    #[allow(unused)]
    width: u32,
    height: u32,
    stride: u32,
    needs_flip: bool,
}

#[windows::core::implement(ISampleGrabberCB)]
struct GrabberCallback {
    state: Arc<GrabberState>,
}

impl ISampleGrabberCB_Impl for GrabberCallback_Impl {
    unsafe fn SampleCB(
        &self,
        _sample_time: f64,
        _sample: *mut core::ffi::c_void,
    ) -> windows_core::Result<()> {
        // SetCallback(_, 1) выбирает BufferCB; SampleCB не вызывается.
        Ok(())
    }

    unsafe fn BufferCB(
        &self,
        _sample_time: f64,
        buffer: *mut u8,
        buffer_len: i32,
    ) -> windows_core::Result<()> {
        if buffer.is_null() || buffer_len <= 0 {
            return Ok(());
        }
        let n = buffer_len as usize;

        {
            let mut scratch = self.state.scratch.lock().unwrap();
            if scratch.len() != n {
                scratch.resize(n, 0);
            }

            if self.state.needs_flip {
                // Bottom-up DIB: строка 0 в буфере — нижняя строка картинки.
                // Копируем построчно в обратном порядке, чтобы downstream
                // получил top-down.
                let stride = self.state.stride as usize;
                let height = self.state.height as usize;
                let dst = scratch.as_mut_ptr();

                for row in 0..height {
                    let src_off = (height - 1 - row) * stride;
                    let dst_off = row * stride;
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            buffer.add(src_off),
                            dst.add(dst_off),
                            stride,
                        );
                    }
                }
            } else {
                // Top-down: одна сплошная копия, как раньше.
                unsafe {
                    std::ptr::copy_nonoverlapping(buffer, scratch.as_mut_ptr(), n);
                }
            }

            // O(1) обмен: pending получает свежий кадр, scratch — старый pending
            // (переиспользуется как буфер следующего кадра).
            let mut pending = self.state.pending.lock().unwrap();
            std::mem::swap(&mut *scratch, &mut *pending);
        }

        self.state.seq.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct Pipeline {
    graph: IGraphBuilder,
    control: IMediaControl,
    callback: ISampleGrabberCB,
    state: Arc<GrabberState>,
    width: u32,
    height: u32,
    stride: u32,
}

pub struct DirectShowCameraBackend {
    #[allow(dead_code)]
    name: String,
    moniker: IMoniker,
    formats: Vec<VideoFormatInfo>,
    graph: Option<IGraphBuilder>,
    control: Option<IMediaControl>,
    callback: Option<ISampleGrabberCB>,
    state: Arc<GrabberState>,
    buffer: Vec<u8>,
    last_seq: u64,
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
    color: ColorSpace,
    seq: u64,
    start: Instant,
}

unsafe impl Send for DirectShowCameraBackend {}

fn ff(code: i32, message: &'static str) -> Error {
    Error::Ffi { code, message }
}

thread_local! {
    static COM_INIT: Cell<bool> = const { Cell::new(false) };
}

fn ensure_com() {
    COM_INIT.with(|init| {
        if !init.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            init.set(true);
        }
    });
}

fn enumerate_devices() -> Result<Vec<(String, IMoniker)>> {
    unsafe {
        let dev_enum: ICreateDevEnum =
            CoCreateInstance(&CLSID_SYSTEM_DEVICE_ENUM, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ff(e.code().0, "CoCreateInstance(CLSID_SystemDeviceEnum)"))?;

        let mut enum_moniker: Option<IEnumMoniker> = None;
        dev_enum
            .CreateClassEnumerator(&CLSID_VIDEO_INPUT_DEVICE_CATEGORY, &mut enum_moniker, 0)
            .map_err(|e| ff(e.code().0, "CreateClassEnumerator"))?;

        let enum_moniker = enum_moniker.ok_or(Error::DeviceNotFound)?;
        let mut devices = Vec::new();

        loop {
            let mut slot: [Option<IMoniker>; 1] = [None];
            let mut fetched: u32 = 0;
            let hr = enum_moniker.Next(&mut slot, Some(&mut fetched));
            if hr.is_err() || fetched == 0 {
                break;
            }

            if let Some(moniker) = slot[0].take() {
                let mut name = String::new();
                let prop_bag: IPropertyBag =
                    match moniker.BindToStorage(None::<&IBindCtx>, None::<&IMoniker>) {
                        Ok(pb) => pb,
                        Err(_) => {
                            devices.push((format!("Camera {}", devices.len() + 1), moniker));
                            continue;
                        }
                    };

                let mut var: VARIANT = std::mem::zeroed();
                if prop_bag
                    .Read(
                        windows::core::w!("FriendlyName"),
                        &mut var as *mut VARIANT,
                        None,
                    )
                    .is_ok()
                {
                    if let Ok(pw) = VariantToStringAlloc(&var as *const VARIANT) {
                        if !pw.is_null() {
                            name = pw
                                .to_string()
                                .unwrap_or_else(|_| format!("Camera {}", devices.len() + 1));
                            CoTaskMemFree(Some(pw.0 as *const _));
                        }
                    }
                }

                devices.push((name, moniker));
            }
        }

        if devices.is_empty() {
            Err(Error::DeviceNotFound)
        } else {
            Ok(devices)
        }
    }
}

fn open_device_moniker(name: &str) -> Result<IMoniker> {
    let devices = enumerate_devices()?;
    tracing::info!(
        "DirectShow: {} device(s) enumerated, selecting '{}'",
        devices.len(),
        if name.is_empty() { "default" } else { name }
    );
    let moniker = devices
        .into_iter()
        .find(|(n, _)| {
            name == "default" || name.is_empty() || n.to_lowercase().contains(&name.to_lowercase())
        })
        .map(|(n, m)| {
            tracing::debug!("DirectShow: selected '{}'", n);
            m
        })
        .ok_or(Error::DeviceNotFound)?;
    Ok(moniker)
}

fn find_output_pin(filter: &IBaseFilter) -> Result<IPin> {
    unsafe {
        let enum_pins = filter.EnumPins().map_err(|e| ff(e.code().0, "EnumPins"))?;
        loop {
            let mut slot: [Option<IPin>; 1] = [None];
            let mut fetched: u32 = 0;
            let hr = enum_pins.Next(&mut slot, Some(&mut fetched));
            if hr.is_err() || fetched == 0 {
                break;
            }
            let Some(pin) = slot[0].take() else {
                continue;
            };
            if let Ok(dir) = pin.QueryDirection() {
                if dir == PINDIR_OUTPUT {
                    return Ok(pin);
                }
            }
        }
        Err(ff(-1, "capture filter has no output pin"))
    }
}

fn free_media_type(mt_ptr: *mut AM_MEDIA_TYPE) {
    unsafe {
        if mt_ptr.is_null() {
            return;
        }
        let mt = &*mt_ptr;
        if !mt.pbFormat.is_null() {
            CoTaskMemFree(Some(mt.pbFormat as *const _));
        }
        CoTaskMemFree(Some(mt_ptr as *const _));
    }
}

fn parse_media_type(mt: &AM_MEDIA_TYPE) -> Option<VideoFormatInfo> {
    if mt.formattype != ds::FORMAT_VIDEO_INFO || mt.pbFormat.is_null() {
        return None;
    }
    let vi = unsafe { &*(mt.pbFormat as *const ds::VideoInfoHeader) };
    let width = vi.bmiHeader.biWidth as u32;
    let height = vi.bmiHeader.biHeight.unsigned_abs();
    if width == 0 || height == 0 {
        return None;
    }
    let format = if mt.subtype == ds::MEDIASUBTYPE_RGB24 {
        PixelFormat::Bgr8
    } else if mt.subtype == ds::MEDIASUBTYPE_NV12 {
        PixelFormat::Nv12
    } else if mt.subtype == ds::MEDIASUBTYPE_YUY2 {
        PixelFormat::Yuyv
    } else if mt.subtype == ds::MEDIASUBTYPE_RGB32 {
        PixelFormat::Bgra8
    } else {
        return None;
    };
    let fps = if vi.AvgTimePerFrame > 0 {
        (10_000_000 / vi.AvgTimePerFrame) as u32
    } else {
        0
    };
    Some(VideoFormatInfo {
        width,
        height,
        fps: fps as usize,
        format,
        color: ColorSpace::Bt709,
    })
}

fn enumerate_formats_on_filter(filter: &IBaseFilter) -> Result<Vec<VideoFormatInfo>> {
    unsafe {
        let pin = find_output_pin(filter)?;
        let stream_config: IAMStreamConfig = pin
            .cast()
            .map_err(|e| ff(e.code().0, "cast to IAMStreamConfig"))?;

        let mut count: i32 = 0;
        let mut caps_size: i32 = 0;
        stream_config
            .GetNumberOfCapabilities(&mut count, &mut caps_size)
            .map_err(|e| ff(e.code().0, "GetNumberOfCapabilities"))?;
        if count <= 0 {
            return Ok(Vec::new());
        }

        let mut formats: Vec<VideoFormatInfo> = Vec::new();
        let mut caps_buf = vec![0u8; caps_size.max(1) as usize];

        for i in 0..count {
            let mut mt_ptr: *mut AM_MEDIA_TYPE = std::ptr::null_mut();
            if stream_config
                .GetStreamCaps(i, &mut mt_ptr, caps_buf.as_mut_ptr())
                .is_err()
            {
                continue;
            }
            if mt_ptr.is_null() {
                continue;
            }
            let mt = &*mt_ptr;
            if let Some(vfi) = parse_media_type(mt) {
                if !formats.contains(&vfi) {
                    formats.push(vfi);
                }
            }
            free_media_type(mt_ptr);
        }

        formats.sort_by(|a, b| {
            a.width
                .cmp(&b.width)
                .then(a.height.cmp(&b.height))
                .then(a.fps.cmp(&b.fps))
        });

        Ok(formats)
    }
}

fn try_set_stream_format(filter: &IBaseFilter, config: &VideoConfig) -> Result<()> {
    unsafe {
        let pin = find_output_pin(filter)?;
        let stream_config: IAMStreamConfig = pin
            .cast()
            .map_err(|e| ff(e.code().0, "cast to IAMStreamConfig"))?;

        let mut count: i32 = 0;
        let mut caps_size: i32 = 0;
        stream_config
            .GetNumberOfCapabilities(&mut count, &mut caps_size)
            .map_err(|e| ff(e.code().0, "GetNumberOfCapabilities"))?;
        if count <= 0 {
            return Err(ff(-1, "no stream caps"));
        }

        let mut caps_buf = vec![0u8; caps_size.max(1) as usize];

        for prefer_rgb in [true, false] {
            for i in 0..count {
                let mut mt_ptr: *mut AM_MEDIA_TYPE = std::ptr::null_mut();
                if stream_config
                    .GetStreamCaps(i, &mut mt_ptr, caps_buf.as_mut_ptr())
                    .is_err()
                {
                    continue;
                }
                if mt_ptr.is_null() {
                    continue;
                }
                let mt = &*mt_ptr;

                let matches_size =
                    mt.formattype == ds::FORMAT_VIDEO_INFO && !mt.pbFormat.is_null() && {
                        let vi = &*(mt.pbFormat as *const ds::VideoInfoHeader);
                        vi.bmiHeader.biWidth as u32 == config.width
                            && vi.bmiHeader.biHeight.unsigned_abs() == config.height
                    };
                let matches_subtype = !prefer_rgb || mt.subtype == ds::MEDIASUBTYPE_RGB24;

                if matches_size && matches_subtype {
                    let hr = stream_config.SetFormat(mt_ptr);
                    free_media_type(mt_ptr);
                    if hr.is_ok() {
                        tracing::info!(
                            "DirectShow: applied stream format {}x{} (rgb_required={})",
                            config.width,
                            config.height,
                            prefer_rgb
                        );
                        return Ok(());
                    }
                } else {
                    free_media_type(mt_ptr);
                }
            }
        }

        Err(ff(-1, "no matching stream format for requested config"))
    }
}

/// Внутренняя часть. Вызывается только из `create_pipeline`, который делает
/// retry при E_FAIL.
fn create_pipeline_once(
    moniker: &IMoniker,
    config: &VideoConfig,
) -> Result<(Pipeline, Vec<VideoFormatInfo>)> {
    unsafe {
        let graph: IGraphBuilder =
            CoCreateInstance(&CLSID_FILTER_GRAPH, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ff(e.code().0, "CoCreateInstance(FilterGraph)"))?;

        let capture_builder: ICaptureGraphBuilder2 =
            CoCreateInstance(&CLSID_CAPTURE_GRAPH_BUILDER2, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ff(e.code().0, "CoCreateInstance(CaptureGraphBuilder2)"))?;
        capture_builder
            .SetFiltergraph(&graph)
            .map_err(|e| ff(e.code().0, "SetFiltergraph"))?;

        let capture_filter: IBaseFilter = moniker
            .BindToObject(None, None)
            .map_err(|e| ff(e.code().0, "IMoniker::BindToObject"))?;
        graph
            .AddFilter(&capture_filter, windows::core::w!("Capture"))
            .map_err(|e| ff(e.code().0, "AddFilter(Capture)"))?;

        let formats = enumerate_formats_on_filter(&capture_filter).unwrap_or_else(|e| {
            tracing::warn!("DirectShow: enumerate_formats_on_filter failed: {:?}", e);
            Vec::new()
        });
        tracing::info!("DirectShow: {} native format(s)", formats.len());
        for f in &formats {
            tracing::debug!(
                "DirectShow: format {}x{} {:?} @ {} fps",
                f.width,
                f.height,
                f.format,
                f.fps
            );
        }

        if config.width > 0 && config.height > 0 {
            match try_set_stream_format(&capture_filter, config) {
                Ok(()) => {}
                Err(e) => tracing::warn!(
                    "DirectShow: could not apply {}x{} before RenderStream: {:?}, using native",
                    config.width,
                    config.height,
                    e
                ),
            }
        }

        let grabber_base: IBaseFilter =
            CoCreateInstance(&CLSID_SAMPLE_GRABBER, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ff(e.code().0, "CoCreateInstance(SampleGrabber)"))?;
        let grabber: ISampleGrabber = grabber_base
            .cast()
            .map_err(|e| ff(e.code().0, "cast to ISampleGrabber"))?;

        let mut mt = AM_MEDIA_TYPE::default();
        mt.majortype = ds::MEDIATYPE_VIDEO;
        mt.subtype = ds::MEDIASUBTYPE_RGB24;
        mt.formattype = ds::FORMAT_VIDEO_INFO;
        grabber
            .SetMediaType(&mt as *const AM_MEDIA_TYPE)
            .map_err(|e| ff(e.code().0, "ISampleGrabber::SetMediaType"))?;
        grabber
            .SetBufferSamples(BOOL::from(true))
            .map_err(|e| ff(e.code().0, "ISampleGrabber::SetBufferSamples"))?;

        graph
            .AddFilter(&grabber_base, windows::core::w!("SampleGrabber"))
            .map_err(|e| ff(e.code().0, "AddFilter(SampleGrabber)"))?;

        let null_renderer: IBaseFilter =
            CoCreateInstance(&CLSID_NULL_RENDERER, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ff(e.code().0, "CoCreateInstance(NullRenderer)"))?;
        graph
            .AddFilter(&null_renderer, windows::core::w!("NullRenderer"))
            .map_err(|e| ff(e.code().0, "AddFilter(NullRenderer)"))?;

        capture_builder
            .RenderStream(
                None,
                &ds::MEDIATYPE_VIDEO,
                &capture_filter,
                &grabber_base,
                &null_renderer,
            )
            .map_err(|e| ff(e.code().0, "RenderStream"))?;

        let mut actual = AM_MEDIA_TYPE::default();
        grabber
            .GetConnectedMediaType(&mut actual)
            .map_err(|e| ff(e.code().0, "GetConnectedMediaType"))?;

        if actual.pbFormat.is_null() {
            return Err(ff(-1, "GetConnectedMediaType returned null pbFormat"));
        }
        if actual.formattype != ds::FORMAT_VIDEO_INFO {
            CoTaskMemFree(Some(actual.pbFormat as *const _));
            return Err(ff(-2, "unexpected formattype (expected FORMAT_VideoInfo)"));
        }

        let video_info = *(actual.pbFormat as *const ds::VideoInfoHeader);
        let bi_height = video_info.bmiHeader.biHeight; // сохраняем знак
        let height = bi_height.unsigned_abs();
        let width = video_info.bmiHeader.biWidth as u32;
        let stride = if video_info.bmiHeader.biSizeImage > 0 && height > 0 {
            video_info.bmiHeader.biSizeImage / height
        } else {
            (width * 3 + 3) & !3
        };

        // DirectShow SampleGrabber для RGB24 отдаёт bottom-up DIB при biHeight > 0:
        // первая строка в буфере — нижняя строка картинки. Для YUV (YUY2/NV12)
        // знак biHeight не влияет — там всегда top-down, флаг останется false.
        let needs_flip = bi_height > 0;

        CoTaskMemFree(Some(actual.pbFormat as *const _));

        tracing::info!(
            "DirectShow: connected {}x{} RGB24->Bgr8 (stride={}, biHeight={}, flip={}, requested={}x{})",
            width,
            height,
            stride,
            bi_height,
            needs_flip,
            config.width,
            config.height
        );

        let frame_size = (stride as usize) * (height as usize);
        let state = Arc::new(GrabberState {
            scratch: Mutex::new(vec![0u8; frame_size]),
            pending: Mutex::new(vec![0u8; frame_size]),
            seq: AtomicU64::new(0),
            width,
            height,
            stride,
            needs_flip,
        });

        let cb_obj = GrabberCallback {
            state: state.clone(),
        };
        let cb_iface: ISampleGrabberCB = cb_obj.into();
        grabber
            .SetCallback(cb_iface.as_raw(), 1) // 1 = BufferCB
            .map_err(|e| ff(e.code().0, "ISampleGrabber::SetCallback"))?;

        let control: IMediaControl = graph
            .cast()
            .map_err(|e| ff(e.code().0, "cast to IMediaControl"))?;
        control
            .Run()
            .map_err(|e| ff(e.code().0, "IMediaControl::Run"))?;

        Ok((
            Pipeline {
                graph,
                control,
                callback: cb_iface,
                state,
                width,
                height,
                stride,
            },
            formats,
        ))
    }
}

/// Обёртка с retry. `E_FAIL` из `RenderStream` — это почти всегда «камера
/// ещё не отпущена после предыдущего teardown». Ждём 100 мс и пробуем снова.
/// При первом успешном вызове (без предыдущего графа) retry не срабатывает.
fn create_pipeline(
    moniker: &IMoniker,
    config: &VideoConfig,
) -> Result<(Pipeline, Vec<VideoFormatInfo>)> {
    const ATTEMPTS: u32 = 3;
    let mut last_err: Option<Error> = None;

    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tracing::warn!(
                "DirectShow: retrying pipeline creation (attempt {}/{})",
                attempt + 1,
                ATTEMPTS
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        match create_pipeline_once(moniker, config) {
            Ok(r) => return Ok(r),
            Err(Error::Ffi { code, .. }) if code == E_FAIL_CODE => {
                last_err = Some(ff(code, "RenderStream (device busy, retrying)"));
            }
            Err(e) => return Err(e),
        }
    }

    Err(last_err.unwrap_or_else(|| ff(E_FAIL_CODE, "RenderStream failed")))
}

impl CameraBackend for DirectShowCameraBackend {
    fn open(name: &str) -> Result<Self> {
        ensure_com();

        let moniker = open_device_moniker(name)?;

        let (pipeline, formats) =
            create_pipeline(&moniker, &VideoConfig::new(0, 0, 0, PixelFormat::Bgr8))?;

        let frame_size = (pipeline.stride as usize) * (pipeline.height as usize);
        let last_seq = pipeline.state.seq.load(Ordering::Acquire);

        Ok(Self {
            name: name.to_string(),
            moniker,
            formats,
            graph: Some(pipeline.graph),
            control: Some(pipeline.control),
            callback: Some(pipeline.callback),
            state: pipeline.state,
            buffer: vec![0u8; frame_size],
            last_seq,
            width: pipeline.width,
            height: pipeline.height,
            stride: pipeline.stride,
            format: PixelFormat::Bgr8,
            color: ColorSpace::Bt709,
            seq: 0,
            start: Instant::now(),
        })
    }

    fn supported_formats(&self) -> Result<Vec<VideoFormatInfo>> {
        Ok(self.formats.clone())
    }
}

impl VideoBackend for DirectShowCameraBackend {
    fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        tracing::info!(
            "DirectShow: reconfigure -> {}x{} {:?}",
            config.width,
            config.height,
            config.format
        );

        if let Some(ctrl) = self.control.take() {
            unsafe {
                let _ = ctrl.Stop();
            }
        }
        self.graph = None;
        self.callback = None;

        let (new_pipeline, new_formats) = create_pipeline(&self.moniker, config)?;

        let frame_size = (new_pipeline.stride as usize) * (new_pipeline.height as usize);
        let last_seq = new_pipeline.state.seq.load(Ordering::Acquire);

        self.graph = Some(new_pipeline.graph);
        self.control = Some(new_pipeline.control);
        self.callback = Some(new_pipeline.callback);
        self.state = new_pipeline.state;
        self.formats = new_formats;

        self.buffer = vec![0u8; frame_size];
        self.last_seq = last_seq;
        self.width = new_pipeline.width;
        self.height = new_pipeline.height;
        self.stride = new_pipeline.stride;
        self.format = PixelFormat::Bgr8;
        self.color = ColorSpace::Bt709;
        self.seq = 0;
        self.start = Instant::now();

        tracing::info!(
            "DirectShow: reconfigured -> {}x{} stride={} RGB24",
            self.width,
            self.height,
            self.stride
        );
        Ok(())
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        if self.control.is_none() {
            return Err(ff(-1, "DirectShow backend closed"));
        }

        let cur_seq = self.state.seq.load(Ordering::Acquire);
        if cur_seq == self.last_seq {
            return Ok(None);
        }
        self.last_seq = cur_seq;

        {
            let mut pending = self.state.pending.lock().unwrap();
            std::mem::swap(&mut self.buffer, &mut *pending);
        }

        let expected = (self.stride as usize) * (self.height as usize);
        if self.buffer.len() < expected {
            return Err(ff(-1, "sample buffer smaller than stride*height"));
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
        Ok(Some(CapturedFrame {
            data: &self.buffer[..expected],
            info,
        }))
    }

    fn reset(&mut self) -> Result<()> {
        let Some(ctrl) = self.control.as_ref() else {
            return Err(ff(-1, "DirectShow backend closed"));
        };
        unsafe {
            ctrl.Stop()
                .map_err(|e| ff(e.code().0, "IMediaControl::Stop"))?;
            ctrl.Run()
                .map_err(|e| ff(e.code().0, "IMediaControl::Run"))?;
        }
        self.last_seq = self.state.seq.load(Ordering::Acquire);
        self.seq = 0;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if let Some(ctrl) = self.control.take() {
            unsafe {
                let _ = ctrl.Stop();
            }
        }
        self.graph = None;
        self.callback = None;
        self.last_seq = self.state.seq.load(Ordering::Acquire);
        Ok(())
    }
}

impl Drop for DirectShowCameraBackend {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    static CAMERA_LOCK: Mutex<()> = Mutex::new(());

    fn camera_guard() -> std::sync::MutexGuard<'static, ()> {
        CAMERA_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn open_first_available_camera() -> Option<DirectShowCameraBackend> {
        DirectShowCameraBackend::open("default").ok()
    }

    #[test]
    fn backend_implements_traits() {
        fn _assert_camera<B: CameraBackend>() {}
        fn _assert_video<B: VideoBackend>() {}
        _assert_camera::<DirectShowCameraBackend>();
        _assert_video::<DirectShowCameraBackend>();
    }

    #[test]
    fn open_unknown_name_returns_error() {
        let r = DirectShowCameraBackend::open("nonexistent_camera_name_xyz_9999");
        assert!(r.is_err());
    }

    #[test]
    fn open_default_resolves_to_some_camera() {
        let _guard = camera_guard();
        match DirectShowCameraBackend::open("default") {
            Ok(_) => {}
            Err(Error::DeviceNotFound) => {
                eprintln!("skipping: no camera available");
            }
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }

    #[test]
    fn configure_rebuilds_graph_with_size() {
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(640, 480, 30, PixelFormat::Nv12);
        backend.configure(&config).expect("configure");

        assert!(backend.width > 0);
        assert!(backend.height > 0);
        assert!(backend.stride >= backend.width * 3);
    }

    #[test]
    fn capture_after_configure() {
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let config = VideoConfig::new(640, 480, 30, PixelFormat::Nv12);
        backend.configure(&config).expect("configure");
        std::thread::sleep(Duration::from_millis(500));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = 0;
        while Instant::now() < deadline && got < 3 {
            match backend.capture_frame() {
                Ok(Some(frame)) => {
                    assert!(frame.info.width > 0);
                    assert!(frame.info.height > 0);
                    assert!(frame.info.stride >= frame.info.width * 3);
                    assert_eq!(frame.info.format, PixelFormat::Bgr8);
                    assert!(!frame.data.is_empty());
                    assert!(
                        frame.data.len() >= (frame.info.stride * frame.info.height) as usize,
                        "buffer too small: {} < stride*height={}",
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
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
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
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
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
    fn capture_does_not_block() {
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        std::thread::sleep(Duration::from_millis(300));

        let start = Instant::now();
        for _ in 0..200 {
            match backend.capture_frame() {
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "capture blocks: {elapsed:?}"
        );
    }

    #[test]
    fn reset_restarts_stream() {
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        std::thread::sleep(Duration::from_millis(300));

        while matches!(backend.capture_frame(), Ok(Some(_))) {}

        backend.reset().expect("reset");
        assert_eq!(backend.seq, 0, "reset must clear sequence counter");

        std::thread::sleep(Duration::from_millis(300));
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = false;
        while Instant::now() < deadline && !got {
            if matches!(backend.capture_frame(), Ok(Some(_))) {
                got = true;
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(got, "frames should flow after reset");
    }

    #[test]
    fn close_stops_stream() {
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        std::thread::sleep(Duration::from_millis(200));

        backend.close().expect("close");

        let r = backend.capture_frame();
        assert!(r.is_err(), "expected error after close");
    }

    #[test]
    fn drop_closes_cleanly() {
        // ← БЫЛО БЕЗ GUARD: могло украсть камеру у теста с guard'ом.
        let _guard = camera_guard();
        for _ in 0..3 {
            let Some(backend) = open_first_available_camera() else {
                eprintln!("skipping: no camera");
                return;
            };
            std::thread::sleep(Duration::from_millis(100));
            drop(backend);
        }
    }

    #[test]
    fn frame_metadata_matches_backend_state() {
        let _guard = camera_guard();
        let Some(mut backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        std::thread::sleep(Duration::from_millis(300));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut info: Option<FrameInfo> = None;
        while Instant::now() < deadline && info.is_none() {
            match backend.capture_frame() {
                Ok(Some(frame)) => info = Some(frame.info),
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => panic!("capture error: {e:?}"),
            }
        }
        let info = info.expect("no frame captured within deadline");

        assert_eq!(info.width, backend.width);
        assert_eq!(info.height, backend.height);
        assert_eq!(info.stride, backend.stride);
        assert_eq!(info.format, backend.format);
        assert_eq!(info.color, backend.color);
    }

    #[test]
    fn supported_formats_returns_ok() {
        let _guard = camera_guard();
        let Some(backend) = open_first_available_camera() else {
            eprintln!("skipping: no camera");
            return;
        };
        let formats = backend
            .supported_formats()
            .expect("supported_formats must not error");
        eprintln!("DirectShow supported_formats: {} entries", formats.len());
        for f in &formats {
            assert!(f.width > 0);
            assert!(f.height > 0);
        }
    }
}

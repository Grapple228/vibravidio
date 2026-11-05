use std::os::fd::OwnedFd;
use std::sync::mpsc::SyncSender;
use std::thread;

use pipewire as pw;
use pipewire::spa::param::ParamType;
use pipewire::spa::pod::{Property, Value, object};
use pipewire::spa::utils::SpaTypes;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::Pod;
use pw::spa::pod::serialize::PodSerializer;
use tracing::{debug, error, warn};

pub struct FrameData {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

pub struct PipeWireReader {
    _thread: thread::JoinHandle<()>,
    quit_tx: pw::channel::Sender<()>,
}

impl PipeWireReader {
    pub fn spawn(
        fd: OwnedFd,
        node_id: u32,
        width: u32,
        height: u32,
        tx: SyncSender<FrameData>,
    ) -> Self {
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();

        let thread = thread::Builder::new()
            .name("voxy-wayland-pw".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_pipewire(fd, node_id, width, height, tx, quit_rx);
                }));
                if let Err(e) = result {
                    let msg = e
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| e.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "<non-string panic>".into());
                    // error! — критично; eprintln! — fallback, если subscriber
                    // не инициализирован (например, тест без tracing_subscriber).
                    error!(msg = %msg, "pipewire thread PANIC");
                    eprintln!("[voxy-wayland] pipewire thread PANIC: {msg}");
                }
            })
            .expect("spawn pipewire thread");

        Self {
            _thread: thread,
            quit_tx,
        }
    }

    pub fn stop(&self) {
        let _ = self.quit_tx.send(());
    }
}

fn run_pipewire(
    fd: OwnedFd,
    node_id: u32,
    _width: u32,
    _height: u32,
    tx: SyncSender<FrameData>,
    quit_rx: pw::channel::Receiver<()>,
) {
    debug!("thread started");
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None).expect("mainloop: pw_main_loop_new failed");
    let context = pw::context::ContextBox::new(&mainloop.loop_(), None)
        .expect("context: pw_context_new failed");
    let core = context
        .connect_fd(fd, None)
        .expect("connect_fd: pw_context_connect_fd failed");
    debug!("core connected to portal fd");

    let stream = pw::stream::StreamBox::new(
        &core,
        "voxy-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .expect("stream: pw_stream_new failed");

    let video_info = std::rc::Rc::new(std::cell::RefCell::new(VideoInfoRaw::new()));

    let _listener = stream
        .add_local_listener_with_user_data((tx, video_info.clone()))
        .state_changed(|_stream, _user_data, old, new| {
            debug!(?old, ?new, "stream state");
        })
        .param_changed(|stream, user_data, id, param| {
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else {
                warn!("param_changed: Format param is None");
                return;
            };
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                warn!("param_changed: parse_format failed");
                return;
            };
            if media_type != pw::spa::param::format::MediaType::Video
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                warn!(?media_type, ?media_subtype, "param_changed: not Video/Raw");
                return;
            }

            let mut info = user_data.1.borrow_mut();
            if let Err(e) = info.parse(param) {
                warn!(error = ?e, "VideoInfoRaw::parse failed");
                return;
            }

            let w = info.size().width;
            let h = info.size().height;
            let fmt = info.format();
            let stride = w * 4;

            debug!(w, h, ?fmt, stride, "negotiated format");

            let buffers_obj = object!(
                SpaTypes::ObjectParamBuffers,
                ParamType::Buffers,
                Property::new(
                    pw::spa::sys::SPA_PARAM_BUFFERS_size,
                    Value::Int((stride * h) as i32),
                ),
                Property::new(
                    pw::spa::sys::SPA_PARAM_BUFFERS_stride,
                    Value::Int(stride as i32),
                ),
                // DataType — битовая маска: SPA_DATA_MemPtr = 1, маска = 2.
                Property::new(pw::spa::sys::SPA_PARAM_BUFFERS_dataType, Value::Int(2i32)),
            );

            let values = match PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &pw::spa::pod::Value::Object(buffers_obj),
            ) {
                Ok(v) => v.0.into_inner(),
                Err(e) => {
                    error!(error = ?e, "Buffers Pod serialize failed");
                    return;
                }
            };
            let buffers_pod = match Pod::from_bytes(&values) {
                Some(p) => p,
                None => {
                    error!("Buffers Pod::from_bytes failed");
                    return;
                }
            };

            if let Err(e) = stream.update_params(&mut [buffers_pod]) {
                error!(error = ?e, "update_params failed");
            } else {
                debug!("update_params OK — negotiation complete");
            }
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let data = &mut datas[0];
            let Some(slice) = data.data() else {
                return;
            };

            let info = user_data.1.borrow();
            let w = info.size().width as u32;
            let h = info.size().height as u32;
            if w == 0 || h == 0 {
                return;
            }
            let stride = w * 4;
            let needed = (stride * h) as usize;
            if slice.len() < needed {
                warn!(len = slice.len(), needed, "process: buffer too small");
                return;
            }

            let frame = FrameData {
                data: slice[..needed].to_vec(),
                width: w,
                height: h,
                stride,
            };
            let _ = user_data.0.try_send(frame);
        })
        .register()
        .expect("register listener");

    let format_obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 30, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 60, denom: 1 }
        ),
    );

    let values = PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(format_obj),
    )
    .expect("EnumFormat Pod serialize failed")
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).expect("EnumFormat Pod::from_bytes failed")];

    stream
        .connect(
            pw::spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .expect("stream.connect failed");
    debug!(node_id, "stream.connect OK, entering mainloop");

    let mainloop_clone = mainloop.clone();
    let _recv = quit_rx.attach(mainloop.loop_(), move |_| {
        debug!("quit signal received");
        mainloop_clone.quit();
    });

    mainloop.run();
    debug!("mainloop exited");
}

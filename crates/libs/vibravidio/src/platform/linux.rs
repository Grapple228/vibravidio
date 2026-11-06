use vibravidio_core::{
    CaptureSource, CapturedFrame, Error, Result, ScreenBackend, ScreenBackendAsync,
    SourceDiscovery, VideoBackend, VideoConfig,
};

pub type DefaultCameraBackend = crate::backend::camera::v4l2::V4l2Backend;

pub enum DefaultScreenBackend {
    Wayland(crate::backend::screen::wayland::WaylandScreenBackend),
    X11(crate::backend::screen::x11::X11ScreenBackend),
}

impl VideoBackend for DefaultScreenBackend {
    fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        match self {
            Self::Wayland(b) => b.configure(config),
            Self::X11(b) => b.configure(config),
        }
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        match self {
            Self::Wayland(b) => b.capture_frame(),
            Self::X11(b) => b.capture_frame(),
        }
    }

    fn reset(&mut self) -> Result<()> {
        match self {
            Self::Wayland(b) => b.reset(),
            Self::X11(b) => b.reset(),
        }
    }

    fn close(&mut self) -> Result<()> {
        match self {
            Self::Wayland(b) => b.close(),
            Self::X11(b) => b.close(),
        }
    }
}

impl ScreenBackend for DefaultScreenBackend {
    fn discover() -> Result<SourceDiscovery> {
        if is_wayland_session() {
            crate::backend::screen::wayland::WaylandScreenBackend::discover()
        } else {
            crate::backend::screen::x11::X11ScreenBackend::discover()
        }
    }

    fn open_source(source: &CaptureSource) -> Result<Self> {
        // Wayland не умеет sync open — портал = D-Bus. Используй
        // `ScreenBackendAsync::open_source_async` из async-контекста.
        if is_wayland_session() {
            return Err(Error::AsyncRequired);
        }
        Ok(Self::X11(
            crate::backend::screen::x11::X11ScreenBackend::open_source(source)?,
        ))
    }
}

impl ScreenBackendAsync for DefaultScreenBackend {
    async fn open_source_async(source: &CaptureSource) -> Result<Self> {
        if is_wayland_session() {
            Ok(Self::Wayland(
                crate::backend::screen::wayland::WaylandScreenBackend::open_source_async(source)
                    .await?,
            ))
        } else {
            // X11: sync под капотом. Default impl `ScreenBackendAsync`
            // делегирует в `Self::open_source`, но пишем явно — читается лучше.
            Self::open_source(source)
        }
    }
}

fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

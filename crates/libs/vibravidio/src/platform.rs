#[cfg(target_os = "linux")]
pub type DefaultCameraBackend = crate::backend::camera::v4l2::V4l2Backend;

#[cfg(target_os = "linux")]
pub enum DefaultScreenBackend {
    Wayland(crate::backend::screen::wayland::WaylandScreenBackend),
    X11(crate::backend::screen::x11::X11ScreenBackend),
}

#[cfg(target_os = "linux")]
impl vibravidio_core::VideoBackend for DefaultScreenBackend {
    fn open(name: &str, kind: vibravidio_core::SourceKind) -> vibravidio_core::Result<Self> {
        if is_wayland_session() {
            match crate::backend::screen::wayland::WaylandScreenBackend::open(name, kind) {
                Ok(b) => return Ok(Self::Wayland(b)),
                Err(e) => {
                    tracing::warn!(
                        "Wayland screen backend failed: {e}. Falling back to X11/XWayland."
                    );
                }
            }
        }
        Ok(Self::X11(
            crate::backend::screen::x11::X11ScreenBackend::open(name, kind)?,
        ))
    }

    fn configure(&mut self, config: &vibravidio_core::VideoConfig) -> vibravidio_core::Result<()> {
        match self {
            Self::Wayland(b) => b.configure(config),
            Self::X11(b) => b.configure(config),
        }
    }

    fn capture_frame(
        &mut self,
    ) -> vibravidio_core::Result<Option<vibravidio_core::CapturedFrame<'_>>> {
        match self {
            Self::Wayland(b) => b.capture_frame(),
            Self::X11(b) => b.capture_frame(),
        }
    }

    fn reset(&mut self) -> vibravidio_core::Result<()> {
        match self {
            Self::Wayland(b) => b.reset(),
            Self::X11(b) => b.reset(),
        }
    }

    fn close(&mut self) -> vibravidio_core::Result<()> {
        match self {
            Self::Wayland(b) => b.close(),
            Self::X11(b) => b.close(),
        }
    }

    fn supported_formats(&self) -> vibravidio_core::Result<Vec<vibravidio_core::VideoFormatInfo>> {
        match self {
            Self::Wayland(b) => b.supported_formats(),
            Self::X11(b) => b.supported_formats(),
        }
    }
}

fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

// #[cfg(target_arch = "wasm32")]
// pub type DefaultCameraBackend = vibravidio_web::WebBackend;

pub const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4;
pub const RING_SIZE: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;
    use vibravidio_core::VideoBackend;

    #[test]
    fn max_frame_bytes_is_1080p_rgba() {
        assert_eq!(MAX_FRAME_BYTES, 1920 * 1080 * 4);
    }

    #[test]
    fn ring_size_is_positive() {
        assert!(RING_SIZE > 0);
    }

    #[test]
    fn default_backend_implements_trait() {
        fn _assert<B: VideoBackend>() {}
        _assert::<DefaultCameraBackend>();
    }
}

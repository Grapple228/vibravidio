#[cfg(target_os = "linux")]
use std::future::Future;

#[cfg(target_os = "linux")]
use vibravidio_core::{
    CaptureSource, CapturedFrame, Result, ScreenBackend, SourceDiscovery, VideoBackend, VideoConfig,
};

#[cfg(target_os = "linux")]
pub type DefaultCameraBackend = crate::backend::camera::v4l2::V4l2Backend;

#[cfg(target_os = "linux")]
pub enum DefaultScreenBackend {
    Wayland(crate::backend::screen::wayland::WaylandScreenBackend),
    X11(crate::backend::screen::x11::X11ScreenBackend),
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
impl ScreenBackend for DefaultScreenBackend {
    fn discover() -> Result<SourceDiscovery> {
        if is_wayland_session() {
            crate::backend::screen::wayland::WaylandScreenBackend::discover()
        } else {
            crate::backend::screen::x11::X11ScreenBackend::discover()
        }
    }

    fn open_source(source: &CaptureSource) -> impl Future<Output = Result<Self>> + Send {
        let source = source.clone();
        async move {
            // Wayland: любой источник требует портал (X11 root там пуст).
            if is_wayland_session() {
                return Ok(Self::Wayland(
                    crate::backend::screen::wayland::WaylandScreenBackend::open_async().await?,
                ));
            }
            // X11: свой источник (сейчас только root, дальше — окна/мониторы).
            Ok(Self::X11(
                crate::backend::screen::x11::X11ScreenBackend::open_source(&source).await?,
            ))
        }
    }
}

#[cfg(target_os = "linux")]
impl DefaultScreenBackend {
    /// Sync fallback для `Screen::run` (тесты, старый API).
    /// На Wayland — spawn + block_on через `WaylandScreenBackend::open_sync`.
    /// В проде используется `ScreenBackend::open_source` (async).
    pub fn open_sync(name: &str) -> Result<Self> {
        if is_wayland_session() {
            match crate::backend::screen::wayland::WaylandScreenBackend::open_sync(name) {
                Ok(b) => return Ok(Self::Wayland(b)),
                Err(e) => tracing::warn!(
                    "Wayland screen backend failed: {e}. Falling back to X11/XWayland."
                ),
            }
        }
        Ok(Self::X11(
            crate::backend::screen::x11::X11ScreenBackend::open_sync(name)?,
        ))
    }
}

fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

pub const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4;
pub const RING_SIZE: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

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

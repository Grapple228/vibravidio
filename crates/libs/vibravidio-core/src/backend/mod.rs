mod camera;
mod screen;

pub use camera::{CameraBackend, CameraBackendAsync};
pub use screen::{
    CaptureSource, CaptureSourceKind, ScreenBackend, ScreenBackendAsync, SourceDiscovery,
};

use crate::{FrameInfo, Result, VideoConfig};

pub struct CapturedFrame<'a> {
    pub data: &'a [u8],
    pub info: FrameInfo,
}

/// Общая часть для camera и screen: configure + capture loop.
/// Камера и экран отличаются тем, как backend открывается (open / discover + open_source),
/// а не тем, как отдаются кадры.
pub trait VideoBackend: Sized + Send + 'static {
    fn configure(&mut self, config: &VideoConfig) -> Result<()>;
    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>>;
    fn reset(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

#[allow(async_fn_in_trait)]
pub trait VideoBackendAsync: VideoBackend {
    async fn configure_async(&mut self, config: &VideoConfig) -> Result<()> {
        self.configure(config)
    }
    async fn capture_frame_async(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        self.capture_frame()
    }
    async fn close_async(&mut self) -> Result<()> {
        self.close()
    }
}

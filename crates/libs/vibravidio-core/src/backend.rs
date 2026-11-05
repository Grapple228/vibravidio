use crate::{FrameInfo, Result, SourceKind, VideoConfig, VideoFormatInfo};

pub struct CapturedFrame<'a> {
    pub data: &'a [u8],
    pub info: FrameInfo,
}

pub trait VideoBackend: Sized + Send + 'static {
    fn open(name: &str, kind: SourceKind) -> Result<Self>;
    fn configure(&mut self, config: &VideoConfig) -> Result<()>;

    fn supported_formats(&self) -> Result<Vec<VideoFormatInfo>> {
        Ok(Vec::new())
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>>;

    fn reset(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

#[allow(async_fn_in_trait)]
pub trait VideoBackendAsync: VideoBackend {
    async fn open_async(name: &str, kind: SourceKind) -> Result<Self>;
    async fn configure_async(&mut self, config: &VideoConfig) -> Result<()>;
    async fn capture_frame_async(&mut self) -> Result<Option<CapturedFrame<'_>>>;
    async fn close_async(&mut self) -> Result<()>;
}

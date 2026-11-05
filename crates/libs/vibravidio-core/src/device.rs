use crate::{
    Result, SourceKind, VideoConfig,
    backend::{CapturedFrame, VideoBackend},
};

pub struct VideoDevice<B: VideoBackend> {
    backend: B,
}

impl<B: VideoBackend> VideoDevice<B> {
    #[inline(always)]
    pub fn open(name: &str, kind: SourceKind) -> Result<Self> {
        Ok(Self {
            backend: B::open(name, kind)?,
        })
    }

    #[inline(always)]
    pub fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        self.backend.configure(config)
    }

    #[inline(always)]
    pub fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        self.backend.capture_frame()
    }

    #[inline(always)]
    pub fn reset(&mut self) -> Result<()> {
        self.backend.reset()
    }

    #[inline(always)]
    pub fn close(&mut self) -> Result<()> {
        self.backend.close()
    }
}

impl<B: VideoBackend> Drop for VideoDevice<B> {
    #[inline(always)]
    fn drop(&mut self) {
        let _ = self.backend.close();
    }
}

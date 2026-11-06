use crate::{Result, VideoBackend, VideoFormatInfo};

pub trait CameraBackend: VideoBackend + Sized {
    /// Открыть устройство по имени. `"default"` — первое доступное.
    fn open(name: &str) -> Result<Self>;

    fn supported_formats(&self) -> Result<Vec<VideoFormatInfo>> {
        Ok(Vec::new())
    }
}

#[allow(async_fn_in_trait)]
pub trait CameraBackendAsync: CameraBackend + Sized {
    async fn open_async(name: &str) -> Result<Self> {
        Self::open(name)
    }
}

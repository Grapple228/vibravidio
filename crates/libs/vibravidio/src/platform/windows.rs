use vibravidio_core::{CameraBackend, CapturedFrame, Result, VideoBackend, VideoConfig};

pub enum DefaultCameraBackend {
    MediaFoundation(crate::backend::camera::mf::MfCameraBackend),
    DirectShow(crate::backend::camera::direct::DirectShowCameraBackend),
}

pub type DefaultScreenBackend = vibravidio_screen_win::WindowsScreenBackend;

impl VideoBackend for DefaultCameraBackend {
    fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        match self {
            Self::MediaFoundation(b) => b.configure(config),
            Self::DirectShow(b) => b.configure(config),
        }
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        match self {
            Self::MediaFoundation(b) => b.capture_frame(),
            Self::DirectShow(b) => b.capture_frame(),
        }
    }

    fn reset(&mut self) -> Result<()> {
        match self {
            Self::MediaFoundation(b) => b.reset(),
            Self::DirectShow(b) => b.reset(),
        }
    }

    fn close(&mut self) -> Result<()> {
        match self {
            Self::MediaFoundation(b) => b.close(),
            Self::DirectShow(b) => b.close(),
        }
    }
}

impl CameraBackend for DefaultCameraBackend {
    fn open(name: &str) -> Result<Self> {
        // 1. Пытаемся открыть через Media Foundation (приоритетный, "родной" для Windows 11)
        match crate::backend::camera::mf::MfCameraBackend::open(name) {
            Ok(backend) => {
                tracing::info!("Camera '{}' opened via Media Foundation", name);
                return Ok(Self::MediaFoundation(backend));
            }
            Err(e) => {
                tracing::warn!(
                    "Media Foundation failed for '{}': {:?}. Falling back to DirectShow...",
                    name,
                    e
                );
            }
        }

        // 2. Если MF не сработал (или камера — виртуальная, как OBS), пробуем DirectShow
        match crate::backend::camera::direct::DirectShowCameraBackend::open(name) {
            Ok(backend) => {
                tracing::info!("Camera '{}' opened via DirectShow (fallback)", name);
                Ok(Self::DirectShow(backend))
            }
            Err(e) => {
                tracing::error!(
                    "Both Media Foundation and DirectShow failed for '{}'. Last error: {:?}",
                    name,
                    e
                );
                Err(e) // возвращаем ошибку DirectShow как финальную
            }
        }
    }
}

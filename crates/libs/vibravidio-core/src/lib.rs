mod backend;
mod config;
mod error;
mod format;
mod frame;

pub use backend::{
    CameraBackend, CameraBackendAsync, CaptureSource, CaptureSourceKind, CapturedFrame,
    ScreenBackend, ScreenBackendAsync, SourceDiscovery, VideoBackend, VideoBackendAsync,
};
pub use config::VideoConfig;
pub use error::{Error, Result};
pub use format::{ColorSpace, FrameInfo, PixelFormat, VideoFormatInfo};
pub use frame::OwnedFrame;

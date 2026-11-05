use crate::PixelFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: PixelFormat,
}

impl VideoConfig {
    pub const fn new(width: u32, height: u32, fps: u32, format: PixelFormat) -> Self {
        Self {
            width,
            height,
            fps,
            format,
        }
    }
}

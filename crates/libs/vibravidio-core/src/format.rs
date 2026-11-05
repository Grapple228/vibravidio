#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    Rgba8,
    Bgra8,
    Rgb8,
    Nv12,
    I420,
    Yuyv,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgba8 | PixelFormat::Bgra8 => 4,
            PixelFormat::Rgb8 => 3,
            PixelFormat::Yuyv => 2,
            PixelFormat::Nv12 | PixelFormat::I420 => 0,
        }
    }

    /// Полный размер буфера одного кадра с учётом stride и planarity.
    /// `stride` — Y-plane stride (bytesperline из драйвера).
    pub const fn buffer_len(self, width: u32, height: u32, stride: u32) -> usize {
        match self {
            PixelFormat::Nv12 | PixelFormat::I420 => (stride * height) as usize * 3 / 2,
            _ => (stride * height) as usize,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Srgb,
    Bt601,
    Bt709,
}

#[derive(Debug, Clone, Copy)]
pub struct VideoFormatInfo {
    pub width: u32,
    pub height: u32,
    pub fps: usize,
    pub format: PixelFormat,
    pub color: ColorSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub color: ColorSpace,
    pub sequence: u64,
    pub timestamp_ns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_pixel_packed_formats() {
        assert_eq!(PixelFormat::Rgba8.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::Bgra8.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::Rgb8.bytes_per_pixel(), 3);
        assert_eq!(PixelFormat::Yuyv.bytes_per_pixel(), 2);
    }

    #[test]
    fn bytes_per_pixel_planar_formats() {
        assert_eq!(PixelFormat::Nv12.bytes_per_pixel(), 0);
        assert_eq!(PixelFormat::I420.bytes_per_pixel(), 0);
    }

    #[test]
    fn buffer_len_packed_640x480() {
        let stride = 640 * 4;
        assert_eq!(
            PixelFormat::Rgba8.buffer_len(640, 480, stride),
            640 * 4 * 480
        );
        assert_eq!(
            PixelFormat::Bgra8.buffer_len(640, 480, stride),
            640 * 4 * 480
        );

        let stride = 640 * 3;
        assert_eq!(
            PixelFormat::Rgb8.buffer_len(640, 480, stride),
            640 * 3 * 480
        );

        let stride = 640 * 2;
        assert_eq!(
            PixelFormat::Yuyv.buffer_len(640, 480, stride),
            640 * 2 * 480
        );
    }

    #[test]
    fn buffer_len_planar_1080p() {
        let stride = 1920;
        let height = 1080;
        let expected = (stride * height) as usize * 3 / 2;
        assert_eq!(PixelFormat::Nv12.buffer_len(1920, height, stride), expected);
        assert_eq!(PixelFormat::I420.buffer_len(1920, height, stride), expected);
    }

    #[test]
    fn buffer_len_with_stride_padding() {
        // V4L2 может выровнять bytesperline до 32 байт.
        // YUYV 1922*2 = 3844, выравнивание до 3872.
        let stride = 3872;
        assert_eq!(
            PixelFormat::Yuyv.buffer_len(1922, 1080, stride),
            stride as usize * 1080
        );
    }

    #[test]
    fn buffer_len_zero_dimensions() {
        assert_eq!(PixelFormat::Rgba8.buffer_len(0, 0, 100), 0);
        assert_eq!(PixelFormat::Rgba8.buffer_len(0, 100, 0), 0);
        assert_eq!(PixelFormat::Nv12.buffer_len(0, 0, 100), 0);
    }

    #[test]
    fn frame_info_is_copy() {
        let a = FrameInfo {
            width: 640,
            height: 480,
            stride: 1280,
            format: PixelFormat::Yuyv,
            color: ColorSpace::Bt601,
            sequence: 42,
            timestamp_ns: 12345,
        };
        let b = a;
        assert_eq!(a.width, b.width);
        assert_eq!(a.sequence, b.sequence);
        assert_eq!(a.timestamp_ns, b.timestamp_ns);
    }

    #[test]
    fn pixel_format_hashable() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        s.insert(PixelFormat::Rgba8);
        s.insert(PixelFormat::Yuyv);
        s.insert(PixelFormat::Rgba8);
        assert_eq!(s.len(), 2);
    }
}

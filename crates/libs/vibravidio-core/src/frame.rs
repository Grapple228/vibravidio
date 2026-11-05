use bytes::Bytes;

use crate::{CapturedFrame, FrameInfo};

#[derive(Clone)]
pub struct OwnedFrame {
    pub data: Bytes,
    pub info: FrameInfo,
}

impl OwnedFrame {
    pub fn from_captured(c: &CapturedFrame<'_>) -> Self {
        Self {
            data: Bytes::copy_from_slice(c.data),
            info: c.info,
        }
    }

    pub fn from_slice(data: &[u8], info: FrameInfo) -> Self {
        Self {
            data: Bytes::copy_from_slice(data),
            info,
        }
    }
}

impl AsRef<[u8]> for OwnedFrame {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorSpace, PixelFormat};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn info() -> FrameInfo {
        FrameInfo {
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Rgba8,
            color: ColorSpace::Srgb,
            sequence: 1,
            timestamp_ns: 100,
        }
    }

    #[test]
    fn from_slice_copies_data() {
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let frame = OwnedFrame::from_slice(&data, info());
        assert_eq!(&frame.data[..], &data[..]);
        assert_eq!(frame.info.width, 2);
    }

    #[test]
    fn from_slice_data_independent_from_source() {
        let mut data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let frame = OwnedFrame::from_slice(&data, info());
        data[0] = 99;
        assert_eq!(frame.data[0], 1, "OwnedFrame must own its data");
    }

    #[test]
    fn from_captured_copies_and_preserves_info() {
        let data = vec![0xABu8; 16];
        let fi = info();
        let captured = CapturedFrame {
            data: &data,
            info: fi,
        };
        let owned = OwnedFrame::from_captured(&captured);

        assert_eq!(&owned.data[..], &data[..]);
        assert_eq!(owned.info.width, fi.width);
        assert_eq!(owned.info.height, fi.height);
        assert_eq!(owned.info.stride, fi.stride);
        assert_eq!(owned.info.format, fi.format);
        assert_eq!(owned.info.color, fi.color);
        assert_eq!(owned.info.sequence, fi.sequence);
        assert_eq!(owned.info.timestamp_ns, fi.timestamp_ns);
    }

    #[test]
    fn clone_shares_bytes_zero_copy() {
        let data = vec![0u8; 1024];
        let frame = OwnedFrame::from_slice(&data, info());
        let clone = frame.clone();
        assert_eq!(frame.data.as_ptr(), clone.data.as_ptr());
    }

    #[test]
    fn as_ref_returns_full_slice() {
        let data = vec![9u8, 8, 7, 6];
        let frame = OwnedFrame::from_slice(&data, info());
        let s: &[u8] = frame.as_ref();
        assert_eq!(s, &[9u8, 8, 7, 6]);
    }

    #[test]
    fn bytes_from_vec_is_zero_copy() {
        let vec = vec![0xCCu8; 1024];
        let ptr = vec.as_ptr();
        let bytes = Bytes::from(vec);
        assert_eq!(bytes.as_ptr(), ptr);
    }

    #[test]
    fn multiple_clones_track_refcount_drops() {
        let drops = Arc::new(AtomicUsize::new(0));

        struct Tracked {
            _data: Bytes,
            drops: Arc<AtomicUsize>,
        }
        impl Drop for Tracked {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }

        let inner = Bytes::from_static(&[0u8; 8]);
        let t = Tracked {
            _data: inner,
            drops: drops.clone(),
        };
        let fi = info();
        let frame = OwnedFrame {
            data: t._data.clone(),
            info: fi,
        };

        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(t);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        drop(frame);
        // Bytes сам не дропает через наш Tracked — Tracked уже упал один раз.
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}

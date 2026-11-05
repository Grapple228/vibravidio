use std::sync::Arc;

use crate::{BufferReader, inner::RingBufferInner};

pub struct BufferWriter<const N: usize, F> {
    pub(crate) inner: Arc<RingBufferInner<N, F>>,
}

impl<const N: usize, F> BufferWriter<N, F> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RingBufferInner::new()),
        }
    }

    pub fn reader(&self) -> BufferReader<N, F> {
        BufferReader {
            inner: self.inner.clone(),
        }
    }

    pub fn write_frame(&self, frame: F) -> core::result::Result<(), F> {
        self.inner.push(frame)
    }

    pub fn available_space(&self) -> usize {
        self.inner.available_space()
    }

    pub fn is_full(&self) -> bool {
        self.available_space() == 0
    }
}

impl<const N: usize, F> Default for BufferWriter<N, F> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_writer_has_full_space() {
        let w = BufferWriter::<4, u32>::new();
        assert_eq!(w.available_space(), 4);
        assert!(!w.is_full());
    }

    #[test]
    fn write_frame_succeeds() {
        let w = BufferWriter::<4, u32>::new();
        assert!(w.write_frame(1).is_ok());
        assert_eq!(w.available_space(), 3);
    }

    #[test]
    fn write_frame_when_full_returns_frame_back() {
        let w = BufferWriter::<2, u32>::new();
        w.write_frame(1).unwrap();
        w.write_frame(2).unwrap();
        assert!(w.is_full());
        match w.write_frame(3) {
            Err(returned) => assert_eq!(returned, 3),
            Ok(()) => panic!("must fail"),
        }
    }

    #[test]
    fn reader_from_writer() {
        let w = BufferWriter::<4, u32>::new();
        let r = w.reader();
        w.write_frame(10).unwrap();
        assert_eq!(r.read_frame(), Some(10));
    }
}

use std::sync::Arc;

use crate::inner::RingBufferInner;

pub struct BufferReader<const N: usize, F> {
    pub(crate) inner: Arc<RingBufferInner<N, F>>,
}

impl<const N: usize, F> BufferReader<N, F> {
    pub fn read_frame(&self) -> Option<F> {
        self.inner.pop()
    }

    pub fn drain_latest(&self) -> Option<F> {
        let mut last = None;
        while let Some(f) = self.inner.pop() {
            last = Some(f);
        }
        last
    }

    pub fn available_data(&self) -> usize {
        self.inner.available_data()
    }

    pub fn is_empty(&self) -> bool {
        self.available_data() == 0
    }
}

#[cfg(test)]
mod tests {
    use crate::create_pair;

    #[test]
    fn empty_reader_returns_none() {
        let (_w, r) = create_pair::<4, u32>();
        assert!(r.read_frame().is_none());
        assert!(r.is_empty());
        assert_eq!(r.available_data(), 0);
    }

    #[test]
    fn read_frame_returns_pushed_value() {
        let (w, r) = create_pair::<4, u32>();
        w.write_frame(42).unwrap();
        assert_eq!(r.read_frame(), Some(42));
    }

    #[test]
    fn drain_latest_returns_most_recent() {
        let (w, r) = create_pair::<8, u32>();
        w.write_frame(10).unwrap();
        w.write_frame(20).unwrap();
        w.write_frame(30).unwrap();
        assert_eq!(r.drain_latest(), Some(30));
        assert!(r.is_empty());
    }

    #[test]
    fn drain_latest_on_empty_returns_none() {
        let (_w, r) = create_pair::<8, u32>();
        assert_eq!(r.drain_latest(), None);
    }

    #[test]
    fn drain_latest_single_item() {
        let (w, r) = create_pair::<8, u32>();
        w.write_frame(42).unwrap();
        assert_eq!(r.drain_latest(), Some(42));
    }
}

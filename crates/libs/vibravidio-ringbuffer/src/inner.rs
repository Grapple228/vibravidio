use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct RingBufferInner<const N: usize, F> {
    data: UnsafeCell<[Option<F>; N]>,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
}

unsafe impl<const N: usize, F: Send> Send for RingBufferInner<N, F> {}
unsafe impl<const N: usize, F: Send> Sync for RingBufferInner<N, F> {}

impl<const N: usize, F> RingBufferInner<N, F> {
    pub fn new() -> Self {
        assert!(N > 0, "ring buffer capacity must be > 0");
        Self {
            data: UnsafeCell::new(std::array::from_fn(|_| None)),
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub fn available_data(&self) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    #[inline]
    pub fn available_space(&self) -> usize {
        N.saturating_sub(self.available_data())
    }

    #[inline]
    pub fn push(&self, frame: F) -> core::result::Result<(), F> {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        if w.wrapping_sub(r) >= N {
            return Err(frame);
        }
        let idx = w % N;
        unsafe {
            let slots = &mut *self.data.get();
            slots[idx] = Some(frame);
        }
        self.write_pos.store(w.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    #[inline]
    pub fn pop(&self) -> Option<F> {
        let r = self.read_pos.load(Ordering::Relaxed);
        let w = self.write_pos.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let idx = r % N;
        let frame = unsafe {
            let slots = &mut *self.data.get();
            slots[idx].take()
        };
        self.read_pos.store(r.wrapping_add(1), Ordering::Release);
        frame
    }
}

impl<const N: usize, F> Default for RingBufferInner<N, F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, F> Drop for RingBufferInner<N, F> {
    fn drop(&mut self) {
        let slots = self.data.get_mut();
        for slot in slots.iter_mut() {
            slot.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    #[derive(Debug)]
    struct Tracked {
        #[allow(dead_code)]
        id: u32,
        drops: Arc<AtomicUsize>,
    }
    impl Tracked {
        fn new(id: u32, drops: &Arc<AtomicUsize>) -> Self {
            Self {
                id,
                drops: drops.clone(),
            }
        }
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn new_ring_is_empty() {
        let r = RingBufferInner::<8, u32>::new();
        assert_eq!(r.available_data(), 0);
        assert_eq!(r.available_space(), 8);
        assert!(r.pop().is_none());
    }

    #[test]
    fn push_then_pop() {
        let r = RingBufferInner::<8, u32>::new();
        assert!(r.push(42).is_ok());
        assert_eq!(r.available_data(), 1);
        assert_eq!(r.pop(), Some(42));
    }

    #[test]
    fn fifo_order() {
        let r = RingBufferInner::<8, u32>::new();
        for i in 0..5 {
            r.push(i).unwrap();
        }
        for i in 0..5 {
            assert_eq!(r.pop(), Some(i));
        }
    }

    #[test]
    fn full_ring_returns_err() {
        const N: usize = 4;
        let r = RingBufferInner::<N, u32>::new();
        for i in 0..N as u32 {
            r.push(i).unwrap();
        }
        assert_eq!(r.available_space(), 0);
        match r.push(999) {
            Err(returned) => assert_eq!(returned, 999),
            Ok(()) => panic!("push must fail when full"),
        }
    }

    #[test]
    fn pop_frees_space() {
        const N: usize = 2;
        let r = RingBufferInner::<N, u32>::new();
        r.push(1).unwrap();
        r.push(2).unwrap();
        assert_eq!(r.available_space(), 0);
        assert_eq!(r.pop(), Some(1));
        assert_eq!(r.available_space(), 1);
        assert!(r.push(3).is_ok());
    }

    #[test]
    fn wrap_around_many_cycles() {
        const N: usize = 4;
        let r = RingBufferInner::<N, u32>::new();
        for cycle in 0..10u32 {
            for i in 0..N as u32 {
                r.push(cycle * 100 + i).unwrap();
            }
            for i in 0..N as u32 {
                assert_eq!(r.pop(), Some(cycle * 100 + i));
            }
        }
    }

    #[test]
    fn interleaved_wrap() {
        const N: usize = 3;
        let r = RingBufferInner::<N, u32>::new();
        for i in 0..100u32 {
            r.push(i).unwrap();
            assert_eq!(r.pop(), Some(i));
        }
    }

    #[test]
    fn capacity_one() {
        let r = RingBufferInner::<1, u32>::new();
        assert!(r.push(1).is_ok());
        assert!(r.push(2).is_err());
        assert_eq!(r.pop(), Some(1));
        assert!(r.push(2).is_ok());
        assert_eq!(r.pop(), Some(2));
    }

    #[test]
    #[should_panic]
    fn zero_capacity_panics() {
        let _ = RingBufferInner::<0, u32>::new();
    }

    #[test]
    fn non_copy_elements() {
        let r = RingBufferInner::<4, String>::new();
        r.push("hello".to_string()).unwrap();
        r.push("world".to_string()).unwrap();
        assert_eq!(r.pop().as_deref(), Some("hello"));
        assert_eq!(r.pop().as_deref(), Some("world"));
    }

    #[test]
    fn remaining_elements_drop_on_ring_drop() {
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let r = RingBufferInner::<8, Tracked>::new();
            r.push(Tracked::new(1, &drops)).unwrap();
            r.push(Tracked::new(2, &drops)).unwrap();
            r.push(Tracked::new(3, &drops)).unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 0);
        }
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn popped_elements_drop_when_caller_drops() {
        let drops = Arc::new(AtomicUsize::new(0));
        let r = RingBufferInner::<8, Tracked>::new();
        r.push(Tracked::new(1, &drops)).unwrap();
        let t = r.pop().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(t);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}

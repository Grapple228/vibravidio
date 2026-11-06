#[cfg(target_os = "linux")]
mod linux;

#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::DefaultCameraBackend;
#[cfg(target_os = "linux")]
pub use linux::DefaultScreenBackend;

#[cfg(windows)]
pub use windows::DefaultCameraBackend;
#[cfg(windows)]
pub use windows::DefaultScreenBackend;

pub const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4;
pub const RING_SIZE: usize = 4;

#[cfg(test)]
mod tests {
    use vibravidio_core::VideoBackend;

    use super::*;

    #[test]
    fn max_frame_bytes_is_1080p_rgba() {
        assert_eq!(MAX_FRAME_BYTES, 1920 * 1080 * 4);
    }

    #[test]
    fn ring_size_is_positive() {
        assert!(RING_SIZE > 0);
    }

    #[test]
    fn default_backend_implements_trait() {
        fn _assert<B: VideoBackend>() {}
        _assert::<DefaultCameraBackend>();
    }
}

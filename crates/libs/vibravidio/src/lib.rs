#[doc = "hidden"]
pub mod core {
    pub use vibravidio_core::*;
}

#[doc = "hidden"]
pub mod ringbuffer {
    pub use vibravidio_ringbuffer::*;
}

#[doc = "hidden"]
pub mod thread {
    pub use vibravidio_thread::*;
}

pub mod devices;

pub mod platform {
    #[cfg(target_os = "linux")]
    pub type DefaultCameraBackend = vibravidio_v4l2::V4l2Backend;

    // #[cfg(target_arch = "wasm32")]
    // pub type DefaultCameraBackend = vibravidio_web::WebBackend;

    pub const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4;
    pub const RING_SIZE: usize = 4;

    #[cfg(test)]
    mod tests {
        use super::*;
        use vibravidio_core::VideoBackend;

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
}

pub mod backend {
    #[cfg(target_os = "linux")]
    pub mod v4l2 {
        pub use vibravidio_v4l2::*;
    }
}

#[cfg(not(target_os = "linux"))]
compile_error!("vibravidio currently supports only Linux");

#[cfg(test)]
mod tests {
    use vibravidio_core::VideoBackend;

    use crate::{backend, platform};

    #[test]
    fn core_reexports_are_accessible() {
        let _ = vibravidio_core::PixelFormat::Rgba8;
        let _ = vibravidio_core::VideoConfig::new(640, 480, 30, vibravidio_core::PixelFormat::Yuyv);
    }

    #[test]
    fn ringbuffer_reexports_are_accessible() {
        let (w, r) = vibravidio_ringbuffer::create_pair::<4, u32>();
        w.write_frame(1).unwrap();
        assert_eq!(r.read_frame(), Some(1));
    }

    #[test]
    fn thread_reexports_are_accessible() {
        use vibravidio_thread::Priority;
        assert_eq!(Priority::Critical.as_str(), "critical");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn backend_v4l2_is_accessible() {
        let _ = backend::v4l2::V4l2Backend::open(
            "/dev/video_nonexistent_999",
            vibravidio_core::SourceKind::Camera,
        );
    }

    #[test]
    fn platform_default_camera_backend_type_exists() {
        // Компиляция — уже проверка.
        fn _assert_type<B: vibravidio_core::VideoBackend>() {}
        _assert_type::<platform::DefaultCameraBackend>();
    }
}

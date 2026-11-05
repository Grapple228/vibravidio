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

pub mod platform;

pub mod backend {
    pub mod camera {
        #[cfg(target_os = "linux")]
        pub mod v4l2 {
            pub use vibravidio_camera_v4l2::*;
        }
    }

    pub mod screen {
        #[cfg(target_os = "linux")]
        pub mod wayland {
            pub use vibravidio_screen_wayland::*;
        }

        #[cfg(target_os = "linux")]
        pub mod x11 {
            pub use vibravidio_screen_x11::*;
        }
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
        let _ = backend::camera::v4l2::V4l2Backend::open(
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

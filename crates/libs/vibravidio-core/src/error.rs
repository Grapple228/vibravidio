use derive_more::derive::From;
use std::ffi::c_int;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, From)]
pub enum Error {
    DeviceNotFound,
    AsyncRequired,
    InvalidConfig,
    UnsupportedFormat,
    PermissionDenied,
    AlreadyRunning,
    SizeChanged {
        old: (u32, u32),
        new: (u32, u32),
    },
    BufferTooSmall {
        needed: usize,
        got: usize,
    },

    Ffi {
        code: c_int,
        message: &'static str,
    },

    #[from]
    Io(std::io::Error),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_io_from_conversion() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn error_display_does_not_panic() {
        let variants = [
            Error::DeviceNotFound,
            Error::AsyncRequired,
            Error::InvalidConfig,
            Error::UnsupportedFormat,
            Error::PermissionDenied,
            Error::AlreadyRunning,
            Error::SizeChanged {
                old: (640, 480),
                new: (1280, 720),
            },
            Error::BufferTooSmall {
                needed: 100,
                got: 50,
            },
            Error::Ffi {
                code: -1,
                message: "test",
            },
        ];
        for e in &variants {
            let s = format!("{e}");
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<Error>();
    }
}

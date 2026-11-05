#![allow(non_camel_case_types)]

use std::ffi::{c_int, c_ulong, c_void};

#[repr(C)]
pub struct PollFd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}

pub const POLLIN: i16 = 0x0001;
pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_SHARED: c_int = 1;
pub const MAP_FAILED: *mut c_void = -1isize as *mut c_void;

unsafe extern "C" {
    pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    pub fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    pub fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
}

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(dir: u32, typ: u8, nr: u8, size: u32) -> c_ulong {
    ((dir << IOC_DIRSHIFT)
        | ((typ as u32) << IOC_TYPESHIFT)
        | ((nr as u32) << IOC_NRSHIFT)
        | (size << IOC_SIZESHIFT)) as c_ulong
}

const fn ior<T>(typ: u8, nr: u8) -> c_ulong {
    ioc(IOC_READ, typ, nr, std::mem::size_of::<T>() as u32)
}
const fn iow<T>(typ: u8, nr: u8) -> c_ulong {
    ioc(IOC_WRITE, typ, nr, std::mem::size_of::<T>() as u32)
}
const fn iowr<T>(typ: u8, nr: u8) -> c_ulong {
    ioc(
        IOC_READ | IOC_WRITE,
        typ,
        nr,
        std::mem::size_of::<T>() as u32,
    )
}
#[allow(dead_code)]
const fn io(typ: u8, nr: u8) -> c_ulong {
    ioc(IOC_NONE, typ, nr, 0)
}

#[repr(C)]
pub struct v4l2_capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct v4l2_pix_format {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub priv_: u32,
    pub flags: u32,
    pub ycbcr_enc: u32,
    pub quantization: u32,
    pub xfer_func: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union v4l2_format_u {
    pub pix: v4l2_pix_format,
    pub _align: [u64; 25],
}

#[repr(C)]
pub struct v4l2_format {
    pub typ: u32,
    pub fmt: v4l2_format_u,
}

#[repr(C)]
pub struct v4l2_requestbuffers {
    pub count: u32,
    pub typ: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub reserved: [u32; 1],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_timecode {
    pub typ: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union v4l2_buffer_m {
    pub offset: u32,
    pub userptr: u64,
    pub planes: *mut c_void,
    pub fd: i32,
}

#[repr(C)]
pub struct v4l2_buffer {
    pub index: u32,
    pub typ: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: [i64; 2],
    pub timecode: v4l2_timecode,
    pub sequence: u32,
    pub memory: u32,
    pub m: v4l2_buffer_m,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}

const _: () = assert!(std::mem::size_of::<v4l2_buffer>() == 88);
const _: () = assert!(std::mem::size_of::<v4l2_format>() == 208);
const _: () = assert!(std::mem::size_of::<v4l2_requestbuffers>() == 20);
const _: () = assert!(std::mem::size_of::<v4l2_capability>() == 104);

const fn v4l2_fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}
pub const V4L2_PIX_FMT_YUYV: u32 = v4l2_fourcc(b'Y', b'U', b'Y', b'V');
#[allow(dead_code)]
pub const V4L2_PIX_FMT_MJPEG: u32 = v4l2_fourcc(b'M', b'J', b'P', b'G');
pub const V4L2_PIX_FMT_RGB24: u32 = v4l2_fourcc(b'R', b'G', b'B', b'3');

pub const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const V4L2_MEMORY_MMAP: u32 = 1;
pub const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
pub const V4L2_CAP_STREAMING: u32 = 0x0400_0000;
pub const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;

pub const VIDIOC_QUERYCAP: c_ulong = ior::<v4l2_capability>(b'V', 0);
pub const VIDIOC_S_FMT: c_ulong = iowr::<v4l2_format>(b'V', 5);
pub const VIDIOC_REQBUFS: c_ulong = iowr::<v4l2_requestbuffers>(b'V', 8);
pub const VIDIOC_QUERYBUF: c_ulong = iowr::<v4l2_buffer>(b'V', 9);
pub const VIDIOC_QBUF: c_ulong = iowr::<v4l2_buffer>(b'V', 15);
pub const VIDIOC_DQBUF: c_ulong = iowr::<v4l2_buffer>(b'V', 17);
pub const VIDIOC_STREAMON: c_ulong = iow::<c_int>(b'V', 18);
pub const VIDIOC_STREAMOFF: c_ulong = iow::<c_int>(b'V', 19);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_kernel_abi_64bit() {
        // Эти assert'ы дублируют const _: () = assert! в модуле выше,
        // но здесь они явно видны как тест.
        assert_eq!(std::mem::size_of::<v4l2_capability>(), 104);
        assert_eq!(std::mem::size_of::<v4l2_format>(), 208);
        assert_eq!(std::mem::size_of::<v4l2_requestbuffers>(), 20);
        assert_eq!(std::mem::size_of::<v4l2_buffer>(), 88);
    }

    #[test]
    fn fourcc_yuyv_is_correct() {
        // YUYV little-endian: 'Y' | 'U'<<8 | 'Y'<<16 | 'V'<<24
        assert_eq!(V4L2_PIX_FMT_YUYV, 0x5659_5559);
    }

    #[test]
    fn fourcc_rgb24_is_correct() {
        // 'R' | 'G'<<8 | 'B'<<16 | '3'<<24
        assert_eq!(V4L2_PIX_FMT_RGB24, 0x3342_4752);
    }

    #[test]
    fn ioctl_numbers_are_nonzero() {
        assert_ne!(VIDIOC_QUERYCAP, 0);
        assert_ne!(VIDIOC_S_FMT, 0);
        assert_ne!(VIDIOC_REQBUFS, 0);
        assert_ne!(VIDIOC_QUERYBUF, 0);
        assert_ne!(VIDIOC_QBUF, 0);
        assert_ne!(VIDIOC_DQBUF, 0);
        assert_ne!(VIDIOC_STREAMON, 0);
        assert_ne!(VIDIOC_STREAMOFF, 0);
    }
}

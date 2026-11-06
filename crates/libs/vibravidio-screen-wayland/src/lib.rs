#[cfg(target_os = "linux")]
mod backend;
#[cfg(target_os = "linux")]
mod pipewire_reader;
#[cfg(target_os = "linux")]
mod portal;

#[cfg(target_os = "linux")]
pub use backend::WaylandScreenBackend;

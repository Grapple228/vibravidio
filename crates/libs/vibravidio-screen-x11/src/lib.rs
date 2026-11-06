#[cfg(target_os = "linux")]
mod backend;
#[cfg(target_os = "linux")]
mod ffi;

#[cfg(target_os = "linux")]
pub use backend::X11ScreenBackend;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;

    #[test]
    fn init_threads_is_idempotent() {
        // Первый вызов инициализирует, второй — no-op через Once.
        // Если XInitThreads недоступен на платформе — open вернёт
        // DeviceNotFound раньше, но ensure_xlib_threads не паникует.
        let _ = std::panic::catch_unwind(|| {
            let _ = X11ScreenBackend::open_sync(":99.99");
            let _ = X11ScreenBackend::open_sync(":99.99");
        });
    }
}

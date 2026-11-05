pub mod thread {
    #[cfg(target_arch = "wasm32")]
    pub use wasm_thread::*;

    #[cfg(not(target_arch = "wasm32"))]
    pub use std::thread::*;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Critical,
    Highest,
    AboveNormal,
    Normal,
}

impl Priority {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Priority::Critical => "critical",
            Priority::Highest => "highest",
            Priority::AboveNormal => "above normal",
            Priority::Normal => "normal",
        }
    }

    #[cfg(target_os = "windows")]
    pub const fn as_value(&self) -> i32 {
        use winapi::um::winbase::{
            THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_HIGHEST, THREAD_PRIORITY_NORMAL,
            THREAD_PRIORITY_TIME_CRITICAL,
        };
        match self {
            Priority::Critical => THREAD_PRIORITY_TIME_CRITICAL as i32,
            Priority::Highest => THREAD_PRIORITY_HIGHEST as i32,
            Priority::AboveNormal => THREAD_PRIORITY_ABOVE_NORMAL as i32,
            Priority::Normal => THREAD_PRIORITY_NORMAL as i32,
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmcssValue {
    Audio,
    Capture,
    Distribution,
    Games,
    Playback,
    ProAudio,
    Video,
    WindowsManager,
    None,
}

#[cfg(target_os = "windows")]
impl MmcssValue {
    pub const fn as_str(&self) -> &'static str {
        match self {
            MmcssValue::Audio => "Audio",
            MmcssValue::Capture => "Capture",
            MmcssValue::Distribution => "Distribution",
            MmcssValue::Games => "Games",
            MmcssValue::Playback => "Playback",
            MmcssValue::ProAudio => "Pro Audio",
            MmcssValue::Video => "Video",
            MmcssValue::WindowsManager => "Windows Manager",
            MmcssValue::None => "None",
        }
    }

    pub const fn needs_enable(&self) -> bool {
        !matches!(self, MmcssValue::None)
    }
}

#[cfg(target_os = "windows")]
type Return = Option<MmcssHandle>;
#[cfg(not(target_os = "windows"))]
type Return = ();

pub fn configure_thread(
    priority: Priority,
    #[cfg(target_os = "windows")] mmcss_value: MmcssValue,
) -> Return {
    set_thread_priority(priority);

    #[cfg(target_os = "windows")]
    {
        return enable_mmcss(mmcss_value);
    }

    #[cfg(not(target_os = "windows"))]
    ()
}

#[cfg(target_os = "windows")]
pub struct MmcssHandle(winapi::um::winnt::HANDLE, #[allow(unused)] u32);

#[cfg(target_os = "windows")]
impl Drop for MmcssHandle {
    fn drop(&mut self) {
        unsafe {
            use winapi::um::avrt::AvRevertMmThreadCharacteristics;
            AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
pub fn enable_mmcss(mmcss_value: MmcssValue) -> Option<MmcssHandle> {
    use winapi::um::avrt::AvSetMmThreadCharacteristicsW;

    if !mmcss_value.needs_enable() {
        return None;
    }
    let task_name = mmcss_value.as_str();
    let mut wide_buf = [0u16; 32];
    let mut i = 0;
    for ch in task_name.encode_utf16() {
        if i < wide_buf.len() - 1 {
            wide_buf[i] = ch;
            i += 1;
        }
    }
    let mut task_index: u32 = 0;
    let handle = unsafe { AvSetMmThreadCharacteristicsW(wide_buf.as_ptr(), &mut task_index) };
    if handle.is_null() {
        tracing::warn!("Failed to enable MMCSS for '{}'", task_name);
        None
    } else {
        tracing::debug!("✅ MMCSS set for '{}'", task_name);
        Some(MmcssHandle(handle, task_index))
    }
}

#[allow(unused)]
pub fn set_thread_priority(priority: Priority) {
    #[cfg(target_os = "windows")]
    unsafe {
        use winapi::um::processthreadsapi::{GetCurrentThread, SetThreadPriority};
        let handle = GetCurrentThread();
        let result = SetThreadPriority(handle, priority.as_value());
        if result == 0 {
            tracing::warn!(
                "Failed to set thread priority: {}",
                std::io::Error::last_os_error()
            );
        } else {
            tracing::debug!("✅ Thread priority set to: {}", priority.as_str());
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = priority;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn priority_as_str() {
        assert_eq!(Priority::Critical.as_str(), "critical");
        assert_eq!(Priority::Highest.as_str(), "highest");
        assert_eq!(Priority::AboveNormal.as_str(), "above normal");
        assert_eq!(Priority::Normal.as_str(), "normal");
    }

    #[test]
    fn priority_copy_and_eq() {
        let p = Priority::Critical;
        let q = p;
        assert_eq!(p, q);
        assert_ne!(Priority::Critical, Priority::Normal);
    }

    #[test]
    fn configure_thread_all_priorities() {
        for p in [
            Priority::Critical,
            Priority::Highest,
            Priority::AboveNormal,
            Priority::Normal,
        ] {
            let _guard = configure_thread(
                p,
                #[cfg(target_os = "windows")]
                MmcssValue::Video,
            );
        }
    }

    #[test]
    fn configure_thread_inside_spawn() {
        let h = thread::spawn(|| {
            let _g = configure_thread(
                Priority::Critical,
                #[cfg(target_os = "windows")]
                MmcssValue::Capture,
            );
            std::thread::sleep(Duration::from_millis(5));
            42u32
        });
        assert_eq!(h.join().unwrap(), 42);
    }

    #[test]
    fn thread_module_spawn_and_join() {
        let h = thread::spawn(|| "hello");
        assert_eq!(h.join().unwrap(), "hello");
    }

    #[test]
    fn thread_module_sleep() {
        let start = Instant::now();
        thread::sleep(Duration::from_millis(10));
        assert!(start.elapsed() >= Duration::from_millis(9));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn mmcss_none_returns_none_handle() {
        let guard = configure_thread(Priority::Normal, MmcssValue::None);
        assert!(guard.is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn mmcss_video_may_return_none_on_some_systems() {
        // Не гарантируется регистрация Video в MMCSS.
        // Проверяем только что не паникует.
        let _ = configure_thread(Priority::Normal, MmcssValue::Video);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn mmcss_audio_always_registered() {
        let guard = configure_thread(Priority::Normal, MmcssValue::Audio);
        assert!(
            guard.is_some(),
            "Audio MMCSS should be always available on Windows"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn mmcss_value_needs_enable() {
        assert!(MmcssValue::Audio.needs_enable());
        assert!(MmcssValue::Video.needs_enable());
        assert!(!MmcssValue::None.needs_enable());
    }
}

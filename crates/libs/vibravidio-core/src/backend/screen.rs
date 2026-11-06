use std::future::Future;

use crate::{Result, VideoBackend};

#[derive(Debug, Clone)]
pub struct CaptureSource {
    /// Opaque ID. UI не парсит. Формат задаёт backend:
    ///   X11: "x11:monitor:0" / "x11:window:0x400001"
    ///   Win: "win:monitor:0x1f3a" / "win:window:0x14001f"
    ///   Wayland: "wayland:token:abc"
    pub id: String,
    pub name: String,
    pub kind: CaptureSourceKind,
    /// RGBA8. Любой размер (обычно 160×90). None — превью недоступно.
    pub thumbnail: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSourceKind {
    Monitor,
    Window,
    /// Wayland: запомненный restore_token из прошлой сессии.
    Remembered,
}

#[derive(Debug, Clone)]
pub enum SourceDiscovery {
    /// Backend сам отдаёт список. UI рисует свой picker. (X11, Windows, macOS)
    Direct(Vec<CaptureSource>),
    /// Портал обязателен (Wayland). UI показывает "Добавить экран" → system dialog.
    /// `saved` — restore tokens для тихого reconnect.
    Portal {
        saved: Vec<CaptureSource>,
        reason: String,
    },
}

pub trait ScreenBackend: VideoBackend + Sized {
    /// Sync discovery. X11 = XQueryTree (~10–50 мс), Win = EnumWindows (~5–20 мс),
    /// Wayland = чтение токенов с диска (мкс).
    fn discover() -> Result<SourceDiscovery>;

    /// Открыть конкретный источник. Async — Wayland делает D-Bus.
    /// Sync backends (X11, Windows) возвращают ready future без `.await` внутри.
    fn open_source(source: &CaptureSource) -> impl Future<Output = Result<Self>> + Send;

    /// Wayland: забрать `restore_token` после успешного open.
    fn take_restore_token(&mut self) -> Option<String> {
        None
    }
}

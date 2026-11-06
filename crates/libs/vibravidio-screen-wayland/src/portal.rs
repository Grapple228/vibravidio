use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use std::os::fd::OwnedFd;

pub struct PortalStream {
    pub fd: OwnedFd,
    pub node_id: u32,
    pub width: u32,
    pub height: u32,
}

/// Синхронный вход в portal flow.
///
/// `ashpd` — async-only, а `VideoBackend::open` синхронный (V4L2/X11
/// тащат tokio в trait не нужно). Поэтому гоняем async в отдельном
/// потоке с `current_thread` runtime. Это работает **в любом** контексте:
/// без рантайма (тесты), внутри `#[tokio::main]` (приложение),
/// в `current_thread` — не нужно проверять `Handle::try_current()`.
///
/// `EnableScreen` — редкое событие (клик), один поток на него невидим.
pub fn open_portal_stream() -> Result<PortalStream, String> {
    std::thread::Builder::new()
        .name("voxy-portal".into())
        .spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("build portal runtime: {e}"))?;
            rt.block_on(open_portal_stream_async())
        })
        .map_err(|e| format!("spawn portal thread: {e}"))?
        .join()
        .map_err(|_| "portal thread panicked".to_string())?
}

pub async fn open_portal_stream_async() -> Result<PortalStream, String> {
    let proxy = Screencast::new().await.map_err(|e| e.to_string())?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|e| e.to_string())?;

    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::EMBEDDED)
                .set_sources(SourceType::MONITOR | SourceType::WINDOW)
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(|e| e.to_string())?;

    let response = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(|e| e.to_string())?
        .response()
        .map_err(|e| e.to_string())?;

    let stream = response
        .streams()
        .first()
        .ok_or_else(|| "no streams selected".to_string())?;

    let node_id = stream.pipe_wire_node_id();
    let (width, height) = stream
        .size()
        .ok_or_else(|| "stream has no size".to_string())?;

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(|e| e.to_string())?;

    Ok(PortalStream {
        fd,
        node_id,
        width: width as u32,
        height: height as u32,
    })
}

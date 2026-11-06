use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Instant;

use tracing::info;
use vibravidio_core::{
    CaptureSource, CapturedFrame, ColorSpace, Error, FrameInfo, PixelFormat, Result, ScreenBackend,
    ScreenBackendAsync, SourceDiscovery, VideoBackend, VideoConfig,
};

use crate::pipewire_reader::{FrameData, PipeWireReader};
use crate::portal::{PortalStream, open_portal_stream_async};

pub struct WaylandScreenBackend {
    receiver: Receiver<FrameData>,
    reader: Option<PipeWireReader>,
    last_frame: Option<FrameData>,
    #[allow(unused)]
    width: u32,
    #[allow(unused)]
    height: u32,
    #[allow(unused)]
    stride: u32,
    format: PixelFormat,
    color: ColorSpace,
    seq: u64,
    start: Instant,
}

impl WaylandScreenBackend {
    fn from_portal(portal: PortalStream) -> Result<Self> {
        info!(
            node_id = portal.node_id,
            width = portal.width,
            height = portal.height,
            "Wayland portal OK"
        );

        let (tx, rx) = sync_channel(2);
        let reader =
            PipeWireReader::spawn(portal.fd, portal.node_id, portal.width, portal.height, tx);

        Ok(Self {
            receiver: rx,
            reader: Some(reader),
            last_frame: None,
            width: portal.width,
            height: portal.height,
            stride: portal.width * 4,
            format: PixelFormat::Bgra8,
            color: ColorSpace::Srgb,
            seq: 0,
            start: Instant::now(),
        })
    }
}

impl ScreenBackend for WaylandScreenBackend {
    fn discover() -> Result<SourceDiscovery> {
        // MVP: без restore token store. Портал показан один раз, дальше — снова.
        Ok(SourceDiscovery::Portal {
            saved: Vec::<CaptureSource>::new(),
            reason: "Wayland session requires xdg-desktop-portal dialog for new sources".into(),
        })
    }

    fn open_source(_source: &CaptureSource) -> Result<Self> {
        // Wayland не умеет sync open — портал = D-Bus + интерактивный диалог.
        // Используй `ScreenBackendAsync::open_source_async`.
        Err(Error::AsyncRequired)
    }

    // take_restore_token — default None (без token store пока).
}

impl ScreenBackendAsync for WaylandScreenBackend {
    async fn open_source_async(_source: &CaptureSource) -> Result<Self> {
        let portal = open_portal_stream_async().await.map_err(|e| {
            tracing::warn!("Wayland portal error: {e}");
            Error::UnsupportedFormat
        })?;
        Self::from_portal(portal)
    }
}

impl VideoBackend for WaylandScreenBackend {
    fn configure(&mut self, _config: &VideoConfig) -> Result<()> {
        // PipeWire сам пейсит: композитор отдаёт кадры с той частотой,
        // с которой обновляется экран. Дополнительный rate-limit не нужен.
        Ok(())
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        match self.receiver.try_recv() {
            Ok(frame) => {
                self.seq += 1;
                self.last_frame = Some(frame);
                let f = self.last_frame.as_ref().unwrap();
                let info = FrameInfo {
                    width: f.width,
                    height: f.height,
                    stride: f.stride,
                    format: self.format,
                    color: self.color,
                    sequence: self.seq,
                    timestamp_ns: self.start.elapsed().as_nanos() as u64,
                };
                Ok(Some(CapturedFrame {
                    data: &f.data,
                    info,
                }))
            }
            Err(_) => Ok(None),
        }
    }

    fn reset(&mut self) -> Result<()> {
        self.close()
    }

    fn close(&mut self) -> Result<()> {
        if let Some(reader) = self.reader.take() {
            reader.stop();
        }
        Ok(())
    }
}

impl Drop for WaylandScreenBackend {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

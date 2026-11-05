use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Instant;

use tracing::info;
use vibravidio_core::{
    CapturedFrame, ColorSpace, Error, FrameInfo, PixelFormat, Result, SourceKind, VideoBackend,
    VideoConfig, VideoFormatInfo,
};

use crate::pipewire_reader::{FrameData, PipeWireReader};
use crate::portal::open_portal_stream;

pub struct WaylandScreenBackend {
    receiver: Receiver<FrameData>,
    reader: Option<PipeWireReader>,
    last_frame: Option<FrameData>,
    width: u32,
    height: u32,
    #[allow(unused)]
    stride: u32,
    format: PixelFormat,
    color: ColorSpace,
    seq: u64,
    start: Instant,
}

impl VideoBackend for WaylandScreenBackend {
    fn open(_name: &str, kind: SourceKind) -> Result<Self> {
        if !matches!(kind, SourceKind::Screen) {
            return Err(Error::InvalidConfig);
        }

        let portal = open_portal_stream().map_err(|e| {
            tracing::warn!("Wayland portal error: {e}");
            Error::UnsupportedFormat
        })?;
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

    // ... остальное тело без изменений
    fn configure(&mut self, _config: &VideoConfig) -> Result<()> {
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

    fn supported_formats(&self) -> Result<Vec<VideoFormatInfo>> {
        Ok(vec![VideoFormatInfo {
            width: self.width,
            height: self.height,
            fps: 30,
            format: PixelFormat::Bgra8,
            color: ColorSpace::Srgb,
        }])
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

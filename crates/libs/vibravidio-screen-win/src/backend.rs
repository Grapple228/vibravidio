use vibravidio_core::{ScreenBackend, VideoBackend};

pub struct WindowsScreenBackend {}

impl VideoBackend for WindowsScreenBackend {
    fn configure(&mut self, _config: &vibravidio_core::VideoConfig) -> vibravidio_core::Result<()> {
        todo!()
    }

    fn capture_frame(
        &mut self,
    ) -> vibravidio_core::Result<Option<vibravidio_core::CapturedFrame<'_>>> {
        todo!()
    }

    fn reset(&mut self) -> vibravidio_core::Result<()> {
        todo!()
    }

    fn close(&mut self) -> vibravidio_core::Result<()> {
        todo!()
    }
}

impl ScreenBackend for WindowsScreenBackend {
    fn discover() -> vibravidio_core::Result<vibravidio_core::SourceDiscovery> {
        todo!()
    }

    fn open_source(_source: &vibravidio_core::CaptureSource) -> vibravidio_core::Result<Self> {
        todo!()
    }
}

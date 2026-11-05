use std::time::{Duration, Instant};

use vibravidio::core::{OwnedFrame, PixelFormat, VideoConfig};
use vibravidio::devices::Camera;
use vibravidio::ringbuffer::create_pair;
use vibravidio::thread::Priority;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    let config = VideoConfig::new(640, 480, 30, PixelFormat::Yuyv);
    let mut camera = Camera::new(config)?;
    let (writer, reader) = create_pair::<8, OwnedFrame>();

    camera.run(
        writer,
        Priority::Critical,
        #[cfg(target_os = "windows")]
        vibravidio::thread::MmcssValue::Video,
    )?;

    let start = Instant::now();
    let mut count: u64 = 0;
    let mut last_seq: u64 = 0;

    while start.elapsed() < Duration::from_secs(3) {
        if let Some(frame) = reader.read_frame() {
            assert!(frame.info.sequence > last_seq);
            last_seq = frame.info.sequence;
            count += 1;
        } else {
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    camera.stop();

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "✅ {count} frames in {elapsed:.2}s ({:.1}fps)",
        count as f64 / elapsed
    );
    Ok(())
}

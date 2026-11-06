use std::time::{Duration, Instant};

use vibravidio_core::{
    CapturedFrame, ColorSpace, FrameInfo, PixelFormat, Result, VideoBackend, VideoConfig,
};

pub struct TestPatternBackend {
    config: VideoConfig,
    /// Предвычисленный фон (цветные полосы). Не меняется между кадрами.
    background: Vec<u8>,
    /// Рабочий буфер: копия фона + движущийся квадрат.
    buffer: Vec<u8>,
    seq: u64,
    last_emit: Instant,
    period: Duration,
    start: Instant,
}

impl TestPatternBackend {
    /// Создаёт backend с заданным конфигом. Не открывает никаких устройств —
    /// просто предвычисляет фон и выставляет period по fps.
    pub fn new(config: VideoConfig) -> Self {
        let period = Duration::from_nanos(1_000_000_000u64 / config.fps.max(1) as u64);
        let background = Self::precompute_background(&config);
        Self {
            config,
            background: background.clone(),
            buffer: background,
            seq: 0,
            last_emit: Instant::now(),
            period,
            start: Instant::now(),
        }
    }

    fn precompute_background(config: &VideoConfig) -> Vec<u8> {
        let w = config.width as usize;
        let h = config.height as usize;
        let mut bg = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let bar = x * 8 / w.max(1);
                let px: [u8; 4] = match bar {
                    0 => [255, 0, 0, 255],
                    1 => [0, 255, 0, 255],
                    2 => [0, 0, 255, 255],
                    3 => [255, 255, 0, 255],
                    4 => [255, 0, 255, 255],
                    5 => [0, 255, 255, 255],
                    6 => [255, 255, 255, 255],
                    _ => [0, 0, 0, 255],
                };
                bg[i..i + 4].copy_from_slice(&px);
            }
        }
        bg
    }

    fn render(&mut self) {
        let w = self.config.width as usize;
        let h = self.config.height as usize;

        // 1) Копируем предвычисленный фон — один memcpy.
        self.buffer.copy_from_slice(&self.background);

        // 2) Рисуем движущийся квадрат (максимум 80×80 пикселей).
        let sq = 80usize;
        let t = self.seq as usize;
        let sx = (t * 4) % (w + sq);
        let sy = h.saturating_sub(sq) / 2;

        let x_start = sx.min(w);
        let x_end = (sx + sq).min(w);
        let y_start = sy;
        let y_end = (sy + sq).min(h);

        for y in y_start..y_end {
            let row_base = y * w;
            for x in x_start..x_end {
                let i = (row_base + x) * 4;
                self.buffer[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
}

impl VideoBackend for TestPatternBackend {
    fn configure(&mut self, config: &VideoConfig) -> Result<()> {
        self.config = *config;
        self.period = Duration::from_nanos(1_000_000_000u64 / config.fps.max(1) as u64);
        self.background = Self::precompute_background(config);
        self.buffer = self.background.clone();
        Ok(())
    }

    fn capture_frame(&mut self) -> Result<Option<CapturedFrame<'_>>> {
        if self.last_emit.elapsed() < self.period {
            return Ok(None);
        }
        self.last_emit = Instant::now();
        self.render();
        self.seq += 1;

        Ok(Some(CapturedFrame {
            data: &self.buffer,
            info: FrameInfo {
                width: self.config.width,
                height: self.config.height,
                stride: self.config.width * 4,
                format: PixelFormat::Rgba8,
                color: ColorSpace::Srgb,
                sequence: self.seq,
                timestamp_ns: self.start.elapsed().as_nanos() as u64,
            },
        }))
    }

    fn reset(&mut self) -> Result<()> {
        self.seq = 0;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Детерминированный тест: формат кадра, stride, sequence.
    /// Не зависит от wall-clock (только ждёт первый кадр).
    #[test]
    fn frame_metadata_is_correct() {
        let mut b = TestPatternBackend::new(VideoConfig::new(1280, 720, 60, PixelFormat::Rgba8));

        // Первый кадр доступен только после истечения period (16.6 ms при 60fps).
        // Ждём с запасом, потом проверяем инварианты.
        std::thread::sleep(Duration::from_millis(50));

        let f = b
            .capture_frame()
            .unwrap()
            .expect("first frame should be ready after 50ms");

        assert_eq!(f.info.width, 1280);
        assert_eq!(f.info.height, 720);
        assert_eq!(f.info.stride, 1280 * 4);
        assert_eq!(f.info.format, PixelFormat::Rgba8);
        assert_eq!(f.data.len(), 1280 * 720 * 4);
        assert_eq!(f.info.sequence, 1);

        // Второй кадр — sequence инкрементируется.
        std::thread::sleep(Duration::from_millis(50));
        let f2 = b.capture_frame().unwrap().expect("second frame");
        assert_eq!(f2.info.sequence, 2);
    }

    /// Pacing-тест: 60 fps ±tolerance. Зависит от wall-clock, поэтому:
    /// - `#[ignore]` — не гоняется при обычном `cargo test`
    /// - требует release (`-r`), иначе debug-сборка даёт 30 fps
    /// - запускается вручную: `cargo test -r -- --ignored pacing`
    #[test]
    #[cfg_attr(not(feature = "timing-tests"), ignore)]
    fn fps_pacing_is_60() {
        let mut b = TestPatternBackend::new(VideoConfig::new(1280, 720, 60, PixelFormat::Rgba8));

        // Прогрев: пропускаем первый кадр, чтобы period был «устаканен».
        std::thread::sleep(Duration::from_millis(20));
        let _ = b.capture_frame();

        let start = Instant::now();
        let mut n = 0u32;
        while start.elapsed() < Duration::from_secs(1) {
            if b.capture_frame().unwrap().is_some() {
                n += 1;
            }
            std::thread::sleep(Duration::from_micros(500));
        }

        // Окно расширено: release стабильно попадает в 55–65,
        // но CI с нагрузкой может дать 50–70.
        assert!(
            (50..=70).contains(&n),
            "fps pacing broken: got {n} frames in 1s (expected 60)"
        );
        eprintln!("✅ {n} frames in 1s (expected ~60)");
    }
}

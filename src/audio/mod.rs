pub mod capture;

use anyhow::Result;
use cpal::Stream;
use tokio::sync::mpsc;

use self::capture::AudioCapture;

pub struct AudioPipeline {
    capture: AudioCapture,
}

impl AudioPipeline {
    pub fn new() -> Result<Self> {
        Ok(Self {
            capture: AudioCapture::new()?,
        })
    }

    /// Start capturing audio. Returns 16-bit LE PCM byte chunks suitable for
    /// streaming directly to transcription WebSocket backends.
    pub fn start(&self) -> Result<(Stream, mpsc::UnboundedReceiver<Vec<u8>>)> {
        let (stream, mut sample_rx) = self.capture.start()?;
        let (tx, rx) = mpsc::unbounded_channel();

        // Conversion runs on a dedicated thread because cpal callbacks are
        // real-time sensitive and must not block on async channel operations
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            loop {
                let samples = match rt.block_on(sample_rx.recv()) {
                    Some(s) => s,
                    None => break,
                };

                if samples.is_empty() {
                    continue;
                }

                // f32 [-1.0, 1.0] → i16 little-endian PCM bytes
                let bytes: Vec<u8> = samples
                    .iter()
                    .flat_map(|&s| {
                        let clamped = s.clamp(-1.0, 1.0);
                        ((clamped * 32767.0) as i16).to_le_bytes()
                    })
                    .collect();
                let _ = tx.send(bytes);
            }
        });

        Ok((stream, rx))
    }
}

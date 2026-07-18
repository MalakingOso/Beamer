pub mod capture;

use anyhow::Result;
use cpal::Stream;
use std::sync::OnceLock;
use tokio::sync::{mpsc, watch};

use self::capture::AudioCapture;

// ─── Live mic level ───────────────────────────────────────────────────────────
// Per-chunk RMS published for recording indicators (the GNOME shell pill's
// waveform). 0.0 = silence, 1.0 = loud speech.

static LEVEL_CHANNEL: OnceLock<(watch::Sender<f32>, watch::Receiver<f32>)> = OnceLock::new();

fn level_channel() -> &'static (watch::Sender<f32>, watch::Receiver<f32>) {
    LEVEL_CHANNEL.get_or_init(|| watch::channel(0.0))
}

/// Subscribe to live mic levels. Receivers see the most recent value only.
pub fn subscribe_levels() -> watch::Receiver<f32> {
    level_channel().1.clone()
}

fn publish_level(level: f32) {
    let _ = level_channel().0.send(level);
}

/// Map raw f32 sample RMS (0.0–1.0 domain) to a display level. Speech RMS
/// rarely exceeds ~0.12 on typical mics, so an 8× gain puts normal speech
/// near full scale.
fn normalize_rms(rms: f32) -> f32 {
    (rms * 8.0).clamp(0.0, 1.0)
}

fn chunk_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod level_tests {
    use super::{chunk_rms, normalize_rms};

    #[test]
    fn silence_is_zero() {
        assert_eq!(normalize_rms(chunk_rms(&[0.0; 64])), 0.0);
        assert_eq!(chunk_rms(&[]), 0.0);
    }

    #[test]
    fn speech_level_rms_lands_near_full_scale() {
        // Constant 0.1 amplitude → RMS 0.1 → 0.8 after gain
        let samples = [0.1_f32; 64];
        let level = normalize_rms(chunk_rms(&samples));
        assert!((level - 0.8).abs() < 1e-4, "got {level}");
    }

    #[test]
    fn loud_input_clamps_to_one() {
        let samples = [0.9_f32; 64];
        assert_eq!(normalize_rms(chunk_rms(&samples)), 1.0);
    }
}

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
                    None => {
                        publish_level(0.0); // stream ended — settle indicators
                        break;
                    }
                };

                if samples.is_empty() {
                    continue;
                }

                publish_level(normalize_rms(chunk_rms(&samples)));

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

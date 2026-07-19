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

/// Convert f32 samples in [-1.0, 1.0] to i16 little-endian PCM bytes.
/// Out-of-range values are clamped before scaling.
pub(crate) fn f32_to_i16_bytes(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            ((clamped * 32767.0) as i16).to_le_bytes()
        })
        .collect()
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

#[cfg(test)]
mod f32_to_i16_bytes_tests {
    use super::f32_to_i16_bytes;

    #[test]
    fn zero_is_zero_bytes() {
        assert_eq!(f32_to_i16_bytes(&[0.0]), vec![0x00, 0x00]);
    }

    #[test]
    fn positive_full_scale() {
        // 1.0 * 32767.0 = 32767 (i16::MAX), LE bytes FF 7F
        assert_eq!(f32_to_i16_bytes(&[1.0]), vec![0xFF, 0x7F]);
    }

    #[test]
    fn negative_full_scale() {
        // -1.0 * 32767.0 = -32767, LE bytes 01 80
        assert_eq!(f32_to_i16_bytes(&[-1.0]), vec![0x01, 0x80]);
    }

    #[test]
    fn out_of_range_positive_clamps_to_positive_full_scale() {
        assert_eq!(f32_to_i16_bytes(&[2.0]), vec![0xFF, 0x7F]);
    }

    #[test]
    fn out_of_range_negative_clamps_to_negative_full_scale() {
        assert_eq!(f32_to_i16_bytes(&[-5.0]), vec![0x01, 0x80]);
    }

    #[test]
    fn multiple_samples_are_concatenated_in_order_little_endian() {
        let bytes = f32_to_i16_bytes(&[0.0, 1.0, -1.0]);
        assert_eq!(
            bytes,
            vec![0x00, 0x00, 0xFF, 0x7F, 0x01, 0x80],
            "expected LE byte pairs concatenated in input order"
        );
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert_eq!(f32_to_i16_bytes(&[]), Vec::<u8>::new());
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
        std::thread::spawn(move || {
            loop {
                let samples = match sample_rx.blocking_recv() {
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
                let bytes = f32_to_i16_bytes(&samples);
                let _ = tx.send(bytes);
            }
        });

        Ok((stream, rx))
    }
}

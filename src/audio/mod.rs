//! Microphone input for a dictation: hotkey → orchestrator → **audio** →
//! transcription → injection.
//!
//! `capture` runs the cpal callback (downmix + resample to 16 kHz mono f32);
//! the chunker thread started by `AudioPipeline::start` converts that to
//! 16-bit LE PCM for the orchestrator and publishes a live level for the
//! recording pill. No VAD, no WAV encoding here. See `agent_docs/audio_pipeline.md`.

pub mod capture;

use anyhow::{Context, Result};
use cpal::Stream;
use std::sync::OnceLock;
use tokio::sync::{mpsc, watch};

use self::capture::AudioCapture;

// Both audio channels are bounded and sized for >=60s of buffering; a stalled
// consumer drops audio rather than growing memory.

/// Outcome of a bounded-channel send. `Full` means backpressure (worth a
/// warning); `Closed` means the receiver went away — normal teardown, dropped
/// silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendOutcome {
    Sent,
    Full,
    Closed,
}

/// Send `payload`, refusing to touch the last `reserve` slots so a sentinel sent
/// with plain `try_send` always has room, even on a saturated channel.
/// Reports `Closed` (never `Full`) once the receiver is gone. Cheap atomic
/// reads only — safe in a real-time audio callback.
pub(crate) fn try_send_reserving<T>(tx: &mpsc::Sender<T>, reserve: usize, payload: T) -> SendOutcome {
    if tx.is_closed() {
        return SendOutcome::Closed;
    }
    if tx.capacity() <= reserve {
        return SendOutcome::Full;
    }
    match tx.try_send(payload) {
        Ok(()) => SendOutcome::Sent,
        Err(mpsc::error::TrySendError::Full(_)) => SendOutcome::Full,
        Err(mpsc::error::TrySendError::Closed(_)) => SendOutcome::Closed,
    }
}

/// Rate-limited warning for a drop on a full channel: first drop, then every
/// 200th. Call ONLY for `Full` — a closed channel is normal teardown.
pub(crate) fn warn_channel_full(count: &mut u64, what: &str) {
    *count += 1;
    if *count == 1 || *count % 200 == 0 {
        tracing::warn!(
            "{} channel full — dropped {} message(s) so far (consumer stalled?)",
            what,
            count
        );
    }
}

// Live mic level: per-chunk RMS (0.0 = silence, 1.0 = loud) for recording indicators.

static LEVEL_CHANNEL: OnceLock<(watch::Sender<f32>, watch::Receiver<f32>)> = OnceLock::new();

fn level_channel() -> &'static (watch::Sender<f32>, watch::Receiver<f32>) {
    LEVEL_CHANNEL.get_or_init(|| watch::channel(0.0))
}

/// Subscribe to live mic levels (feeds the recording pill). Receivers see the
/// most recent value only.
pub fn subscribe_levels() -> watch::Receiver<f32> {
    level_channel().1.clone()
}

fn publish_level(level: f32) {
    let _ = level_channel().0.send(level);
}

/// dB window for the meter: -55 dBFS (below room noise) reads as 0,
/// -12 dBFS (loud speech) reads as full scale.
const LEVEL_FLOOR_DBFS: f32 = -55.0;
const LEVEL_CEIL_DBFS: f32 = -12.0;

/// Exponent applied after the dB mapping in `normalize_rms`.
const LEVEL_CONTRAST: f32 = 1.4;

/// Map raw f32 sample RMS to a 0.0–1.0 display level.
///
/// Don't "boost" this with a multiplier: ordinary speech saturates at 1.0 and
/// the meter stops moving. The `powf` is monotonic with fixed endpoints, so it
/// only stretches contrast in the range where speech lives.
fn normalize_rms(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    let dbfs = 20.0 * rms.log10();
    let level = ((dbfs - LEVEL_FLOOR_DBFS) / (LEVEL_CEIL_DBFS - LEVEL_FLOOR_DBFS)).clamp(0.0, 1.0);
    level.powf(LEVEL_CONTRAST)
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
    fn normal_speech_leaves_headroom_to_show_variation() {
        // RMS 0.1 must not already read as full scale.
        let level = normalize_rms(chunk_rms(&[0.1_f32; 64]));
        assert!(level > 0.6, "normal speech must be clearly visible, got {level}");
        assert!(
            level < 0.95,
            "normal speech must not consume the whole meter, got {level}"
        );
    }

    #[test]
    fn distinct_speech_levels_produce_distinct_readings() {
        let quiet = normalize_rms(0.03);
        let mid = normalize_rms(0.06);
        let loud = normalize_rms(0.12);
        assert!(quiet < mid && mid < loud, "{quiet} {mid} {loud}");
        assert!(
            mid - quiet > 0.05 && loud - mid > 0.05,
            "steps must be visible on a 26px bar, got {quiet} {mid} {loud}"
        );
    }

    #[test]
    fn quiet_speech_still_registers() {
        let level = normalize_rms(0.005);
        assert!(level > 0.1, "quiet speech should be visible, got {level}");
        assert!(level < 0.5, "…but clearly below normal speech, got {level}");
    }

    #[test]
    fn a_quiet_room_reads_as_silence() {
        assert_eq!(normalize_rms(0.0005), 0.0, "room tone must not drive the meter");
    }
}

#[cfg(test)]
mod channel_backpressure_tests {
    use super::{try_send_reserving, warn_channel_full, SendOutcome};
    use tokio::sync::mpsc;

    /// A stalled consumer saturating the data path via `try_send_reserving`
    /// must never consume the reserved headroom, so a later sentinel sent
    /// with plain `try_send` always has room.
    #[test]
    fn sentinel_survives_full_buffer_via_reserved_headroom() {
        let capacity = 8;
        let reserve = 2;
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(capacity);

        // Stalled consumer: nothing ever calls rx.recv().
        let mut sent = 0u32;
        let mut dropped = 0u32;
        let mut dropped_counter: u64 = 0;
        for i in 0..20u8 {
            match try_send_reserving(&tx, reserve, vec![i]) {
                SendOutcome::Sent => sent += 1,
                SendOutcome::Full => {
                    dropped += 1;
                    warn_channel_full(&mut dropped_counter, "test");
                }
                SendOutcome::Closed => panic!("receiver is still alive in this test"),
            }
        }

        // Data fills exactly `capacity - reserve` slots, then refuses.
        assert_eq!(sent, (capacity - reserve) as u32, "data should stop at the reserve boundary");
        assert_eq!(dropped, 20 - sent, "everything past the reserve boundary should be dropped");
        assert_eq!(dropped_counter, dropped as u64);

        let sentinel_result = tx.try_send(Vec::new());
        assert!(
            sentinel_result.is_ok(),
            "sentinel must have room via the untouched reserved headroom, even though the channel is saturated with data"
        );

        let mut received = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            received.push(msg);
        }
        assert_eq!(received.len(), sent as usize + 1, "all sent data plus the sentinel should be present");
        assert!(
            received.iter().any(|m| m.is_empty()),
            "sentinel (empty Vec) must be present among delivered messages"
        );
        assert!(received.last().unwrap().is_empty(), "sentinel should be the last delivered message");
    }

    /// A closed channel must report `Closed`, not `Full`, so callers stay silent.
    #[test]
    fn closed_channel_reports_closed_not_full_and_does_not_warn() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(4);
        drop(rx);

        let mut dropped_counter: u64 = 0;
        let mut warned = false;
        match try_send_reserving(&tx, 0, vec![1]) {
            SendOutcome::Closed => {}
            SendOutcome::Full => {
                warn_channel_full(&mut dropped_counter, "test");
                warned = true;
            }
            SendOutcome::Sent => panic!("receiver was dropped — send must not succeed"),
        }
        assert!(!warned, "a closed channel must never trigger the 'full — consumer stalled?' warn path");
        assert_eq!(dropped_counter, 0, "closed-channel sends must not be counted as drops");
    }
}

#[cfg(test)]
mod f32_to_i16_bytes_tests {
    use super::f32_to_i16_bytes;

    #[test]
    fn samples_convert_to_clamped_little_endian_pairs_in_order() {
        // 0, full scale both ways, then out-of-range values clamped to full scale.
        assert_eq!(
            f32_to_i16_bytes(&[0.0, 1.0, -1.0, 2.0, -5.0]),
            vec![0x00, 0x00, 0xFF, 0x7F, 0x01, 0x80, 0xFF, 0x7F, 0x01, 0x80]
        );
    }
}

/// PCM chunk channel capacity: 60s at the 5ms cpal callback floor (200/sec).
/// No sentinel travels here, so no reserved headroom.
const CHUNK_CHANNEL_CAPACITY: usize = 12_000;

/// The default input device plus the chunker thread that turns its samples
/// into PCM chunks.
pub struct AudioPipeline {
    capture: AudioCapture,
}

impl AudioPipeline {
    pub fn new() -> Result<Self> {
        Ok(Self {
            capture: AudioCapture::new()?,
        })
    }

    /// Start capturing. Returns the cpal `Stream` (keep it alive; dropping it
    /// stops capture) and a receiver of 16-bit LE, 16 kHz, mono PCM chunks.
    /// An empty chunk marks a device error; capture may continue after it.
    pub fn start(&self) -> Result<(Stream, mpsc::Receiver<Vec<u8>>)> {
        let (stream, mut sample_rx) = self.capture.start()?;
        let (tx, rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);

        // A plain thread, off the async runtime, keeps PCM conversion and level
        // metering out of the real-time cpal callback. Named for thread lists
        // and crash dumps.
        std::thread::Builder::new()
            .name("beamer-audio-chunker".into())
            .spawn(move || {
                let mut dropped_chunks: u64 = 0;
                loop {
                    let samples = match sample_rx.blocking_recv() {
                        Some(s) => s,
                        None => {
                            publish_level(0.0);
                            break;
                        }
                    };

                    if samples.is_empty() {
                        // Capture-error sentinel: drop the meter to zero rather
                        // than freezing the pill waveform at its last value,
                        // and pass the (empty) marker on so the orchestrator
                        // can tell the user.
                        publish_level(0.0);
                        let _ = tx.try_send(Vec::new());
                        continue;
                    }

                    publish_level(normalize_rms(chunk_rms(&samples)));

                    let bytes = f32_to_i16_bytes(&samples);
                    match tx.try_send(bytes) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            warn_channel_full(&mut dropped_chunks, "PCM chunk");
                        }
                        // Consumer gone: normal teardown, not backpressure.
                        Err(mpsc::error::TrySendError::Closed(_)) => {}
                    }
                }
            })
            .context("failed to spawn the audio chunker thread")?;

        Ok((stream, rx))
    }
}

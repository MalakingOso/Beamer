pub mod capture;

use anyhow::Result;
use cpal::Stream;
use std::sync::OnceLock;
use tokio::sync::{mpsc, watch};

use self::capture::AudioCapture;

// Bounded channels throughout the audio/transcription path are sized for
// >=60s of buffering; see each construction site for its capacity math.

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

/// Map raw f32 sample RMS to a 0.0–1.0 display level.
///
/// ⚠️ Do not "boost" this with a multiplier: it saturates ordinary speech at
/// 1.0 and the meter stops moving. The `powf` below is monotonic with fixed
/// endpoints, so it only stretches contrast in the range where speech lives.
const LEVEL_CONTRAST: f32 = 1.4;

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
    fn silence_is_zero() {
        assert_eq!(normalize_rms(chunk_rms(&[0.0; 64])), 0.0);
        assert_eq!(chunk_rms(&[]), 0.0);
    }

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

    #[test]
    fn loud_input_clamps_to_one() {
        let samples = [0.9_f32; 64];
        assert_eq!(normalize_rms(chunk_rms(&samples)), 1.0);
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

    /// Negative control: without the reserve a saturated channel refuses the
    /// sentinel too.
    #[test]
    fn without_reserve_a_saturated_channel_drops_the_sentinel_too() {
        let capacity = 4;
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(capacity);
        for i in 0..capacity {
            assert_eq!(try_send_reserving(&tx, 0, vec![i as u8]), SendOutcome::Sent);
        }
        assert!(tx.try_send(Vec::new()).is_err(), "sentinel has nowhere to go without reserved headroom");
    }

    /// A full channel with a live receiver must report `Full`, so callers warn.
    #[test]
    fn full_channel_with_live_receiver_reports_full_and_warns() {
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(2);
        assert_eq!(try_send_reserving(&tx, 0, vec![1]), SendOutcome::Sent);
        assert_eq!(try_send_reserving(&tx, 0, vec![2]), SendOutcome::Sent);

        let mut dropped_counter: u64 = 0;
        match try_send_reserving(&tx, 0, vec![3]) {
            SendOutcome::Full => warn_channel_full(&mut dropped_counter, "test"),
            other => panic!(
                "a genuinely full channel must be treated as Full and trigger the warn path, got {other:?}"
            ),
        }
        assert_eq!(dropped_counter, 1);
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

    /// Even under the reserve threshold, a dropped receiver reports `Closed`.
    #[test]
    fn closed_channel_reports_closed_even_under_reserve_threshold() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(4);
        drop(rx);

        assert_eq!(try_send_reserving(&tx, 10, vec![1]), SendOutcome::Closed);
    }

    /// Plain `try_send` must distinguish `Full` from `Closed` the same way.
    #[test]
    fn plain_try_send_error_distinguishes_full_from_closed() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(1);
        assert!(tx.try_send(vec![1]).is_ok());
        match tx.try_send(vec![2]) {
            Err(mpsc::error::TrySendError::Full(_)) => {}
            other => panic!("expected Full, got {other:?}"),
        }

        drop(rx);
        match tx.try_send(vec![3]) {
            Err(mpsc::error::TrySendError::Closed(_)) => {}
            other => panic!("expected Closed, got {other:?}"),
        }
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
        assert_eq!(f32_to_i16_bytes(&[1.0]), vec![0xFF, 0x7F]);
    }

    #[test]
    fn negative_full_scale() {
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

/// Converted-PCM chunk channel capacity: same worst-case cadence as the sample
/// channel (60s * 200 msgs/sec at the 5ms cpal callback floor) = 12_000.
/// No sentinel travels here, so no reserved headroom.
const CHUNK_CHANNEL_CAPACITY: usize = 12_000;

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
    pub fn start(&self) -> Result<(Stream, mpsc::Receiver<Vec<u8>>)> {
        let (stream, mut sample_rx) = self.capture.start()?;
        let (tx, rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);

        // Dedicated thread: cpal callbacks are real-time sensitive and must not
        // block on async channel operations. Named like the other
        // long-lived threads, so crash dumps and thread lists say what it is.
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
                        // than freezing the pill waveform at its last value.
                        publish_level(0.0);
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
            .expect("failed to spawn audio chunker thread");

        Ok((stream, rx))
    }
}

pub mod capture;

use anyhow::Result;
use cpal::Stream;
use std::sync::OnceLock;
use tokio::sync::{mpsc, watch};

use self::capture::AudioCapture;

// ─── Bounded channel helpers ──────────────────────────────────────────────────
// Shared by the mic-capture path (`capture.rs`, this file) and the
// transcription-side channels (`transcription/mod.rs` + backends,
// `orchestrator.rs`) — all of them replaced unbounded mpsc channels with
// bounded ones sized for >=60s of buffering (TB.10). See each channel's
// construction site for its capacity math.

/// Outcome of a bounded-channel send attempt. Callers use this to decide
/// whether a failed send is worth a "consumer stalled?" warning: `Full`
/// means the channel is genuinely backed up and the message was dropped as
/// a result, while `Closed` means the receiver side has gone away (e.g.
/// normal session teardown) and the message was dropped because there is
/// nobody left to receive it — not because anything is stalled. Pre-branch
/// code silently dropped sends on a closed channel (`let _ = tx.send(...)`);
/// this preserves that behavior while still surfacing genuine backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendOutcome {
    Sent,
    Full,
    Closed,
}

/// Attempt to send `payload` on `tx`, but only if doing so leaves at least
/// `reserve` slots free. This is how sentinel-carrying channels guarantee a
/// rare, must-not-drop sentinel (e.g. an end-of-audio marker) always has
/// room to `try_send`, even when a stalled consumer has let the data path
/// saturate the rest of the channel: the data path calls this function
/// (which refuses to touch the last `reserve` slots), while the sentinel is
/// sent with a plain `tx.try_send(..)` that only ever competes for the
/// untouched reserved slots.
///
/// Returns `SendOutcome::Closed` (rather than `Full`) whenever the receiver
/// has been dropped, even if the reserve threshold would otherwise have
/// refused the send — a closed channel is never "stalled," so callers
/// should not warn on it.
///
/// `Sender::capacity()` and `Sender::is_closed()` are both cheap atomic
/// reads (not locks), so this never blocks or spins — safe to call from a
/// real-time audio callback.
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

/// Rate-limited warning for a message dropped because a bounded channel was
/// full (consumer stalled). Logs the first drop immediately, then every
/// 200th thereafter, so a sustained stall produces one log line up front
/// and periodic reminders rather than flooding the log. Only increments a
/// plain counter and occasionally calls `tracing::warn!` — no allocation,
/// locking, or blocking beyond what `tracing::warn!` itself does.
///
/// Call this ONLY for `SendOutcome::Full` / `TrySendError::Full` — a closed
/// channel is normal teardown, not backpressure, and should be dropped
/// silently (see `SendOutcome`).
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

/// Quietest RMS worth showing at all, and the level that reads as full scale.
///
/// A dB window rather than a gain, because loudness is perceived
/// logarithmically and a linear gain spends almost its whole range on the
/// quietest sounds. -55 dBFS sits below a typical room's noise floor; -12 dBFS
/// is loud speech just short of clipping.
const LEVEL_FLOOR_DBFS: f32 = -55.0;
const LEVEL_CEIL_DBFS: f32 = -12.0;

/// Map raw f32 sample RMS (0.0–1.0 domain) to a 0.0–1.0 display level.
///
/// ⚠️ **Do not "boost" this with a multiplier.** The previous version was
/// `(rms * 30.0).clamp(0.0, 1.0).sqrt()`, which saturated at RMS 0.033
/// (-29.5 dBFS). Ordinary speech runs 0.03–0.15 RMS, so essentially every
/// chunk containing speech pinned to exactly 1.0 and the waveform showed no
/// variation between talking and silence. The `sqrt` was added to prevent
/// precisely that, but it ran *after* the clamp — where `sqrt(1.0) == 1.0` —
/// so it could only lift quiet input, dragging the noise floor up too.
///
/// Mapping in the dB domain keeps speech inside a *range* instead of pinned
/// at its top, which is the whole point of a level meter.
///
/// Ordinary speech (0.03–0.15 RMS, see above) lands at ~0.57–0.90 on this
/// scale, which left the waveform looking flat between a normal and a loud
/// voice. The `powf` below is not the multiplier boost warned against above
/// — it's monotonic and fixes both endpoints (0 stays 0, 1 stays 1), so it
/// can't reintroduce the old saturate-at-1.0 bug. An exponent > 1 stretches
/// values apart in the upper range where real speech lives, while pushing
/// near-floor noise even closer to 0.
const LEVEL_CONTRAST: f32 = 1.4;

fn normalize_rms(rms: f32) -> f32 {
    if rms <= 0.0 {
        // Also keeps log10 away from -inf.
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
        // The regression this replaces asserted RMS 0.1 → *exactly* 1.0, and
        // passing was the bug: with the meter already at full scale there is
        // nothing left for louder speech to move.
        let level = normalize_rms(chunk_rms(&[0.1_f32; 64]));
        assert!(level > 0.6, "normal speech must be clearly visible, got {level}");
        assert!(
            level < 0.95,
            "normal speech must not consume the whole meter, got {level}"
        );
    }

    #[test]
    fn distinct_speech_levels_produce_distinct_readings() {
        // The property the old implementation could not satisfy at any gain,
        // and the one the user actually noticed was missing. Any single
        // saturating transform collapses these onto each other.
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
        // A quiet mic must not read as silence, or the pill looks broken for
        // anyone whose input gain is low.
        let level = normalize_rms(0.005);
        assert!(level > 0.1, "quiet speech should be visible, got {level}");
        assert!(level < 0.5, "…but clearly below normal speech, got {level}");
    }

    #[test]
    fn a_quiet_room_reads_as_silence() {
        // The other half of the reported symptom: with the old gain, room
        // noise was lifted toward full scale, so silence looked like speech.
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

    /// Core guarantee behind TB.10's sentinel-safety design: a "stalled
    /// consumer" (nobody draining) that saturates the data path via
    /// `try_send_reserving` can never consume the reserved headroom, so a
    /// sentinel sent afterwards with a plain `try_send` always has room —
    /// proving end-of-audio (and the capture-error sentinel) delivery under
    /// a full buffer.
    #[test]
    fn sentinel_survives_full_buffer_via_reserved_headroom() {
        let capacity = 8;
        let reserve = 2;
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(capacity);

        // Simulate a stalled consumer: nothing ever calls rx.recv(), so the
        // data path below will fill the channel until only the reserved
        // slots are left, then start dropping.
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

        // Data fills exactly `capacity - reserve` slots, then every further
        // attempt is refused before it ever touches the reserved headroom.
        assert_eq!(sent, (capacity - reserve) as u32, "data should stop at the reserve boundary");
        assert_eq!(dropped, 20 - sent, "everything past the reserve boundary should be dropped");
        assert_eq!(dropped_counter, dropped as u64);

        // The end-of-audio sentinel must still get through: the reserved
        // slots were never touched by the data path above.
        let sentinel_result = tx.try_send(Vec::new());
        assert!(
            sentinel_result.is_ok(),
            "sentinel must have room via the untouched reserved headroom, even though the channel is saturated with data"
        );

        // Drain and confirm the sentinel (an empty Vec) was actually
        // delivered, not silently dropped.
        let mut received = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            received.push(msg);
        }
        assert_eq!(received.len(), sent as usize + 1, "all sent data plus the sentinel should be present");
        assert!(
            received.iter().any(|m| m.is_empty()),
            "sentinel (empty Vec) must be present among delivered messages"
        );
        // Sentinel was sent last, after all data, so it must be the last
        // message a consumer would observe.
        assert!(received.last().unwrap().is_empty(), "sentinel should be the last delivered message");
    }

    /// Without the reserve, a fully-saturated channel would also refuse the
    /// sentinel — this is the negative control proving the reserve (not
    /// just "the channel wasn't literally full yet") is what saves it.
    #[test]
    fn without_reserve_a_saturated_channel_drops_the_sentinel_too() {
        let capacity = 4;
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(capacity);
        for i in 0..capacity {
            assert_eq!(try_send_reserving(&tx, 0, vec![i as u8]), SendOutcome::Sent);
        }
        // Channel is now completely full (reserve = 0 reserved no slots).
        assert!(tx.try_send(Vec::new()).is_err(), "sentinel has nowhere to go without reserved headroom");
    }

    /// A genuinely full channel (consumer alive but stalled) must report
    /// `Full`, so callers warn — this is the positive control for the
    /// Full/Closed distinction below.
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

    /// A closed channel (receiver dropped — normal teardown, e.g. end of a
    /// recording session) must report `Closed`, not `Full`, so callers do
    /// NOT emit a "channel full — consumer stalled?" warning for what is
    /// actually just normal shutdown. This is the regression test for the
    /// bug this commit fixes.
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

    /// Same distinction, but for the reserve-threshold short-circuit path:
    /// even when `capacity() <= reserve` would normally look like `Full`,
    /// a dropped receiver must still report `Closed` so callers don't warn.
    #[test]
    fn closed_channel_reports_closed_even_under_reserve_threshold() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(4);
        drop(rx);

        // reserve >= capacity would normally hit the "Full" short-circuit
        // for a live receiver, but a closed channel must win that check.
        assert_eq!(try_send_reserving(&tx, 10, vec![1]), SendOutcome::Closed);
    }

    /// Plain `try_send` (used directly for sentinels, and by the realtime
    /// transcript-forwarding `send_event` closures) must let callers
    /// distinguish `TrySendError::Full` from `TrySendError::Closed` the
    /// same way `try_send_reserving` does, since both call sites match on
    /// the raw error rather than going through `try_send_reserving`.
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

/// Bounded capacity for the converted-PCM chunk channel. Each message here
/// is produced 1:1 from an incoming raw-sample chunk off `capture.rs`'s
/// sample channel (converted synchronously on the dedicated thread below —
/// see that loop), so it shares the same worst-case cadence: 60s * 200
/// msgs/sec (5ms cpal callback floor) = 12_000. See
/// `capture::SAMPLE_CHANNEL_CAPACITY` for the full derivation.
///
/// No sentinel travels on this channel — the upstream error-callback empty
/// Vec is filtered out by `if samples.is_empty() { continue; }` below before
/// it would ever reach `tx`, so no reserved headroom is needed here.
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

        // Conversion runs on a dedicated thread because cpal callbacks are
        // real-time sensitive and must not block on async channel operations
        std::thread::spawn(move || {
            let mut dropped_chunks: u64 = 0;
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
                match tx.try_send(bytes) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn_channel_full(&mut dropped_chunks, "PCM chunk");
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Consumer (orchestrator) has dropped its receiver —
                        // normal teardown, not backpressure. Silent drop
                        // matches pre-branch `let _ = tx.send(...)` behavior.
                    }
                }
            }
        });

        Ok((stream, rx))
    }
}

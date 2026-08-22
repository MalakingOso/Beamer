//! Leaf helpers for driving one recording session's shutdown sequence.
//!
//! Split out of `mod.rs` to keep that file under the 500-line limit; these are
//! the pieces shared by the realtime and batch recording loops that don't need
//! access to the orchestrator's UI signals beyond the status log.

use dioxus::prelude::*;
use tokio::sync::mpsc;

use crate::audio::{try_send_reserving, warn_channel_full, SendOutcome};
use crate::transcription;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Why a recording loop exited. Both reasons share the same teardown; they
/// differ only in whether trailing audio is still worth collecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopReason {
    /// The user released the hotkey (or the hotkey channel closed).
    UserStop,
    /// The mic stopped delivering audio — device unplugged, capture error.
    AudioLost,
}

/// How long to keep capturing after the hotkey is released, so a trailing word
/// isn't clipped off the end of the transcript.
pub(super) const TAIL_CAPTURE_MS: u64 = 400;

/// How long to wait for the backend's closing transcripts after committing.
pub(super) const FINAL_TRANSCRIPT_TIMEOUT_MS: u64 = 2000;

/// Forward any audio still arriving for `TAIL_CAPTURE_MS`, then return.
///
/// Exits early if the audio channel closes rather than spinning on a closed
/// `recv()` — which returns `None` immediately — until the deadline.
pub(super) async fn stream_tail_audio(
    audio_rx: &mut mpsc::Receiver<Vec<u8>>,
    audio_tx: &mpsc::Sender<Vec<u8>>,
    audio_drop_count: &mut u64,
) {
    let tail = tokio::time::Instant::now() + tokio::time::Duration::from_millis(TAIL_CAPTURE_MS);
    loop {
        tokio::select! {
            chunk = audio_rx.recv() => {
                match chunk {
                    Some(bytes) if !bytes.is_empty() => {
                        match try_send_reserving(audio_tx, transcription::AUDIO_SENTINEL_RESERVE, bytes) {
                            SendOutcome::Sent => {}
                            SendOutcome::Full => warn_channel_full(audio_drop_count, "Realtime audio_tx"),
                            // WebSocket reader task exited (e.g. connection
                            // dropped) — nobody left to receive; normal
                            // teardown, not backpressure.
                            SendOutcome::Closed => {}
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(tail) => break,
        }
    }
}

/// Collect any audio still arriving for `TAIL_CAPTURE_MS` into `buffer`
/// (the batch path's equivalent of `stream_tail_audio`).
pub(super) async fn buffer_tail_audio(
    audio_rx: &mut mpsc::Receiver<Vec<u8>>,
    buffer: &mut Vec<u8>,
) {
    let tail = tokio::time::Instant::now() + tokio::time::Duration::from_millis(TAIL_CAPTURE_MS);
    loop {
        tokio::select! {
            chunk = audio_rx.recv() => {
                match chunk {
                    Some(bytes) => buffer.extend_from_slice(&bytes),
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(tail) => break,
        }
    }
}

/// Send the end-of-audio sentinel (an empty `Vec`) telling the backend to
/// commit/finalize.
///
/// `AUDIO_SENTINEL_RESERVE` slots are never touched by the data path (see the
/// `try_send_reserving` calls), so this should always succeed while the
/// WebSocket reader task is alive, even if the channel was saturated with
/// audio moments ago. A `Closed` error just means that task already exited
/// (e.g. the WebSocket dropped) — there's no backend left to finalize, so it's
/// dropped silently rather than surfaced as an error.
pub(super) fn send_commit_sentinel(
    audio_tx: &mpsc::Sender<Vec<u8>>,
    status_log: &mut Signal<StatusLog>,
) {
    match audio_tx.try_send(Vec::new()) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::error!(
                "End-of-audio sentinel dropped — audio_tx channel full despite {} reserved slots; backend will not receive a finalize signal",
                transcription::AUDIO_SENTINEL_RESERVE
            );
            log_status(
                status_log,
                LogLevel::Error,
                "Failed to send end-of-audio signal — transcript may be incomplete",
            );
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {}
    }
}

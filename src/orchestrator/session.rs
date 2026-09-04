//! Helpers for one recording session's shutdown sequence, shared by the
//! realtime and batch loops.

use dioxus::prelude::*;
use tokio::sync::mpsc;

use crate::audio::{try_send_reserving, warn_channel_full, SendOutcome};
use crate::transcription;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Why a recording loop exited. Only `UserStop` still collects trailing audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopReason {
    /// The user released the hotkey (or the hotkey channel closed).
    UserStop,
    /// The mic stopped delivering audio.
    AudioLost,
    /// The backend's transcript stream closed mid-recording.
    TranscriptLost,
}

/// Why the backend's transcript stream ended. A stream that closes having
/// delivered nothing is a rejected connection, not a finished one: ElevenLabs
/// accepts the WebSocket upgrade before validating the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClosedStream {
    /// The backend was transcribing, then the stream ended.
    Interrupted,
    /// The stream closed before delivering anything at all.
    NeverStarted,
}

/// Whether an event proves the backend was really transcribing. `Info` and
/// `Error` do not count: the reader emits `Info("WebSocket closed")` on the
/// way out even when the server rejected the key.
pub(super) fn proves_session_live(kind: &transcription::TranscriptKind) -> bool {
    use transcription::TranscriptKind as K;
    matches!(kind, K::Final | K::Partial | K::SessionStarted(_))
}

impl ClosedStream {
    pub(super) fn classify(saw_live_event: bool) -> Self {
        if saw_live_event {
            Self::Interrupted
        } else {
            Self::NeverStarted
        }
    }

    /// A failure the user can act on outranks one they can only note.
    pub(super) fn level(self) -> LogLevel {
        match self {
            Self::Interrupted => LogLevel::Warn,
            Self::NeverStarted => LogLevel::Error,
        }
    }

    pub(super) fn message(self, backend: &str) -> String {
        match self {
            Self::Interrupted => {
                format!("{backend} stopped sending transcripts mid-recording")
            }
            Self::NeverStarted => format!(
                "{backend} closed the connection without transcribing anything — \
                 check the API key and your plan's limits"
            ),
        }
    }
}

/// How long to keep capturing after the hotkey is released, so a trailing word
/// isn't clipped off the end of the transcript.
pub(super) const TAIL_CAPTURE_MS: u64 = 400;

/// How long to wait for the backend's closing transcripts after committing.
pub(super) const FINAL_TRANSCRIPT_TIMEOUT_MS: u64 = 2000;

/// Forward audio still arriving for `TAIL_CAPTURE_MS`. Breaks early if the
/// channel closes instead of spinning on a `recv()` that returns `None` at once.
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
                            // Reader task exited: normal teardown, not backpressure.
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

/// Batch equivalent of `stream_tail_audio`: collect trailing audio into `buffer`.
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

/// Send the end-of-audio sentinel (empty `Vec`) telling the backend to commit.
/// Reserved slots keep room for it even on a saturated channel; `Closed` just
/// means the reader task already exited, so it is dropped silently.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::TranscriptKind;

    #[test]
    fn a_stream_that_closed_before_any_transcript_is_reported_as_a_failure() {
        // A bad key is only distinguishable from a working session here.
        let closed = ClosedStream::classify(false);
        assert_eq!(closed, ClosedStream::NeverStarted);
        assert_eq!(closed.level(), LogLevel::Error);
        assert!(
            closed.message("ElevenLabs").contains("API key"),
            "a user who cannot see the cause cannot fix it: {}",
            closed.message("ElevenLabs")
        );
    }

    #[test]
    fn a_stream_that_died_mid_recording_is_a_warning_not_a_key_problem() {
        let closed = ClosedStream::classify(true);
        assert_eq!(closed, ClosedStream::Interrupted);
        assert_eq!(closed.level(), LogLevel::Warn);
        assert!(
            !closed.message("ElevenLabs").contains("API key"),
            "the key demonstrably worked — blaming it would send the user the wrong way"
        );
    }

    #[test]
    fn a_closing_info_event_does_not_count_as_a_live_session() {
        // The reader emits this even for a rejected key; counting it would
        // hide every rejected connection as an interruption.
        assert!(!proves_session_live(&TranscriptKind::Info(
            "WebSocket closed".into()
        )));
        assert!(!proves_session_live(&TranscriptKind::Error("nope".into())));
    }

    #[test]
    fn real_transcription_activity_counts_as_a_live_session() {
        assert!(proves_session_live(&TranscriptKind::Final));
        assert!(proves_session_live(&TranscriptKind::Partial));
        assert!(proves_session_live(&TranscriptKind::SessionStarted("s1".into())));
    }
}

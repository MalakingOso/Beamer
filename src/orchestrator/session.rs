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

/// Why a recording loop exited. All reasons share the same teardown; they
/// differ only in whether trailing audio is still worth collecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopReason {
    /// The user released the hotkey (or the hotkey channel closed).
    UserStop,
    /// The mic stopped delivering audio — device unplugged, capture error.
    AudioLost,
    /// The backend's transcript stream closed while we were still recording.
    ///
    /// Only `UserStop` collects trailing audio, which is right here for the
    /// same reason it is right for `AudioLost`: there is nothing left on the
    /// other end to send it to.
    TranscriptLost,
}

/// Why the backend's transcript stream ended.
///
/// A stream that closes without ever having delivered anything is the
/// signature of a **rejected connection**, not a finished one — and telling
/// the two apart is the only way that failure can be named for the user.
///
/// This matters because of a specific ElevenLabs behaviour, measured rather
/// than assumed: it accepts the WebSocket upgrade *before* validating the key,
/// answering `101 Switching Protocols` in ~130ms and only dropping the socket
/// some seconds later. Mistral answers `401` at the handshake, so its failures
/// never reach the recording loop at all. Without this distinction a bad
/// ElevenLabs key looks exactly like a healthy session that happened to
/// transcribe nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClosedStream {
    /// The backend was transcribing, then the stream ended.
    Interrupted,
    /// The stream closed before delivering anything at all.
    NeverStarted,
}

/// Whether an event proves the backend was really transcribing.
///
/// `Info` and `Error` deliberately do not count. The ElevenLabs reader emits
/// an `Info("WebSocket closed")` on its way out **even when the server
/// rejected the key and sent nothing else**, so counting every event would
/// make a rejected connection indistinguishable from a working one — which is
/// the whole thing [`ClosedStream`] exists to distinguish.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::TranscriptKind;

    #[test]
    fn a_stream_that_closed_before_any_transcript_is_reported_as_a_failure() {
        // The rejected-key signature. ElevenLabs answers 101 to the upgrade
        // before it validates anything, so this is the only point at which a
        // bad key becomes distinguishable from a working session.
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
        // The trap this rule exists for. `elevenlabs_realtime`'s reader emits
        // Info("WebSocket closed") on its way out even when the server
        // rejected the key and sent nothing else. Counting it would make every
        // rejected connection look like an interrupted one.
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

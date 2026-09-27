//! Helpers for one recording session's shutdown sequence.

use tokio::sync::mpsc;

/// Why a recording loop exited. Only `UserStop` still collects trailing audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopReason {
    /// The user released the hotkey (or the hotkey channel closed).
    UserStop,
    /// The mic stopped delivering audio.
    AudioLost,
}

/// How long to keep capturing after the hotkey is released, so a trailing word
/// isn't clipped off the end of the transcript.
pub(super) const TAIL_CAPTURE_MS: u64 = 400;

/// Batch equivalent of capturing trailing audio: collect it into `buffer`.
/// Breaks early if the channel closes instead of spinning on a `recv()` that
/// returns `None` at once.
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

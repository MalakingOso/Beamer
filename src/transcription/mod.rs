mod elevenlabs_batch;
mod elevenlabs_realtime;
mod voxtral_batch;
mod voxtral_realtime;
mod wav;

pub use elevenlabs_batch::transcribe_batch;
pub use elevenlabs_realtime::start_realtime_session as start_elevenlabs_session;
pub use voxtral_batch::transcribe_batch as transcribe_voxtral_batch;
pub use voxtral_realtime::start_realtime_session as start_voxtral_session;

use std::sync::OnceLock;
use tokio::sync::mpsc;

/// Lazy-initialized shared HTTP client for all transcription backends.
/// Avoids rebuilding connection pools and TLS contexts on every request.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Discriminant for transcript events. Both backends normalize their
/// wire-format messages into this shared enum.
#[derive(Debug, Clone)]
pub enum TranscriptKind {
    /// Intermediate hypothesis (displayed in overlay, not injected)
    Partial,
    /// Committed transcript (injected into the focused window)
    Final,
    SessionStarted(String),
    Error(String),
    Info(String),
}

/// A transcription event from the realtime backend.
#[derive(Debug, Clone)]
pub struct TranscriptEvent {
    pub text: String,
    pub kind: TranscriptKind,
}

/// Handle to a running WebSocket transcription session.
/// Send PCM audio bytes via `audio_tx`; receive transcript events via `transcript_rx`.
/// Sending an empty `Vec<u8>` signals the backend to commit/finalize.
pub struct RealtimeSession {
    pub audio_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub transcript_rx: mpsc::UnboundedReceiver<TranscriptEvent>,
}

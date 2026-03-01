mod elevenlabs_realtime;
mod voxtral_realtime;

pub use elevenlabs_realtime::start_realtime_session as start_elevenlabs_session;
pub use voxtral_realtime::start_realtime_session as start_voxtral_session;

use tokio::sync::mpsc;

/// What kind of transcript event this is.
#[derive(Debug, Clone)]
pub enum TranscriptKind {
    Partial,
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

/// Realtime session handle
pub struct RealtimeSession {
    pub audio_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub transcript_rx: mpsc::UnboundedReceiver<TranscriptEvent>,
}

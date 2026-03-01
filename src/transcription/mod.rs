pub mod elevenlabs_batch;
pub mod elevenlabs_realtime;

use anyhow::Result;
use async_trait::async_trait;
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

#[async_trait]
pub trait TranscriptionBackend: Send + Sync {
    /// Transcribe a complete WAV audio buffer
    async fn transcribe_batch(
        &self,
        audio: Vec<u8>,
        language: &str,
        vocab: &[String],
    ) -> Result<String>;

    /// Start a realtime streaming session. Returns None if not supported.
    async fn start_realtime_session(&self, language: &str) -> Result<Option<RealtimeSession>>;
}

pub fn create_backend(
    backend_name: &str,
    api_key: String,
) -> Box<dyn TranscriptionBackend> {
    match backend_name {
        "elevenlabs_realtime" => Box::new(elevenlabs_realtime::ElevenLabsRealtime::new(api_key)),
        _ => Box::new(elevenlabs_batch::ElevenLabsBatch::new(api_key)),
    }
}

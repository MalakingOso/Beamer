mod elevenlabs_batch;
mod elevenlabs_realtime;
pub(crate) mod keyterms;
mod voxtral_batch;
mod voxtral_realtime;
mod wav;

pub use elevenlabs_batch::transcribe_batch;
pub use elevenlabs_realtime::start_realtime_session as start_elevenlabs_session;
pub use voxtral_batch::transcribe_batch as transcribe_voxtral_batch;
pub use voxtral_realtime::start_realtime_session as start_voxtral_session;

use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;

/// How long to wait for TCP + TLS to a backend's API host. A stalled connect
/// would strand the recording loop, which owns the hotkey receiver.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling on one batch request, end to end. Generous: it covers uploading a
/// whole recording and transcribing it, so it bounds a stall, not latency.
pub(crate) const BATCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait for a realtime WebSocket handshake. Also covers the startup
/// warmup preconnect, which runs while the main window is still hidden.
pub(crate) const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Outbound PCM channel capacity (`RealtimeSession::audio_tx`): same worst-case
/// cadence as the mic-capture path, 60s * 200 msgs/sec = 12_000.
pub(crate) const AUDIO_CHANNEL_CAPACITY: usize = 12_000;

/// Slots withheld from PCM data on `audio_tx` so the end-of-audio sentinel
/// always has room to `try_send`, even on a saturated channel.
pub(crate) const AUDIO_SENTINEL_RESERVE: usize = 4;

/// Inbound transcript-event channel capacity (`RealtimeSession::transcript_rx`).
/// Driven by the provider's push cadence (~10Hz); sized for 20Hz: 60s * 20 = 1_200.
pub(crate) const TRANSCRIPT_CHANNEL_CAPACITY: usize = 1_200;

/// Shared HTTP client for all transcription backends.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Warm DNS, TLS and the connection pool for a batch backend's API host.
///
/// An unauthenticated GET to the API root pays the one-time connection costs
/// with no billable side effect; any status counts as success — only the
/// transport matters here.
pub async fn preconnect_batch_host(backend: &str) -> anyhow::Result<()> {
    let url = match backend {
        "voxtral_batch" => "https://api.mistral.ai/",
        _ => "https://api.elevenlabs.io/",
    };
    http_client()
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    Ok(())
}

/// Transcript events; both backends normalize their wire messages into this.
#[derive(Debug, Clone)]
pub enum TranscriptKind {
    /// Intermediate hypothesis (overlay only, never injected).
    Partial,
    /// Committed transcript (injected or noted).
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
    pub audio_tx: mpsc::Sender<Vec<u8>>,
    pub transcript_rx: mpsc::Receiver<TranscriptEvent>,
}

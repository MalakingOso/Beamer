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
use std::time::Duration;
use tokio::sync::mpsc;

/// How long to wait for TCP + TLS to a backend's API host.
///
/// None of the four backends had any timeout at all before this: a stalled
/// connect left the orchestrator's recording loop awaiting forever, and
/// because that loop owns the hotkey receiver, *no further hotkey was ever
/// processed*. The app stayed painted but stopped responding to dictation.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling on one batch transcription request, end to end.
///
/// Deliberately generous rather than snappy: this covers uploading a whole
/// recording *and* transcribing it, so it is sized for a long dictation on a
/// slow link. It exists to bound a stall, not to enforce a latency target —
/// erring long costs a slow transcript, erring short costs the transcript.
pub(crate) const BATCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait for a realtime WebSocket handshake.
///
/// Applied inside each `start_*_session`, so the startup warmup preconnect is
/// covered too — that one runs behind the splash while the main window is
/// still hidden, where a stall means an app that never appears at all.
pub(crate) const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Bounded capacity for a realtime backend's outbound PCM channel
/// (`RealtimeSession::audio_tx`). `orchestrator.rs` forwards
/// `AudioPipeline`'s PCM chunks here 1:1, so this shares the mic-capture
/// path's worst-case cadence — see `audio::capture::SAMPLE_CHANNEL_CAPACITY`
/// for the full derivation: 60s * 200 msgs/sec (5ms cpal callback floor) =
/// 12_000.
pub(crate) const AUDIO_CHANNEL_CAPACITY: usize = 12_000;

/// Slots withheld from ordinary PCM data on `audio_tx` so the end-of-audio
/// sentinel (an empty `Vec<u8>` that tells the backend to finalize/commit)
/// always has room to `try_send`, even when a stalled backend/WebSocket has
/// let the data path saturate the rest of the channel. Sent at most once or
/// twice per recording session, so a small reserve is ample.
pub(crate) const AUDIO_SENTINEL_RESERVE: usize = 4;

/// Bounded capacity for a realtime backend's inbound transcript-event
/// channel (`RealtimeSession::transcript_rx`). Unlike the audio channels
/// above, this is driven by the ASR provider's own push cadence, not the
/// mic's callback cadence: ElevenLabs Scribe v2 realtime and Voxtral mini
/// realtime typically emit partial-transcript updates every 100-300ms
/// (<=10Hz). We size for a conservative 20Hz upper bound to leave margin:
///
///   60s * 20/sec = 1_200
pub(crate) const TRANSCRIPT_CHANNEL_CAPACITY: usize = 1_200;

/// Lazy-initialized shared HTTP client for all transcription backends.
/// Avoids rebuilding connection pools and TLS contexts on every request.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            // A builder failure here means TLS init failed; the plain
            // constructor is no more likely to work, but falling back keeps a
            // transcription attempt possible instead of panicking at startup.
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Warm DNS, TLS and the shared client's connection pool for a batch
/// backend's API host, without starting a transcription.
///
/// Batch backends have no session to open, so warmup used to fall through to
/// opening a *realtime* WebSocket instead — a different, metered product from
/// the one the user selected, opened and discarded on every single launch.
/// An unauthenticated GET to the API root pays the same one-time connection
/// costs with no billable side effect; the response is discarded and any
/// status (including 401/404) counts as success, since only the transport
/// matters here.
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
    pub audio_tx: mpsc::Sender<Vec<u8>>,
    pub transcript_rx: mpsc::Receiver<TranscriptEvent>,
}

//! Cloud speech-to-text: hotkey → orchestrator → audio → **transcription** →
//! injection.
//!
//! Each backend is a plain async fn taking a whole recording's PCM and
//! returning text; there is no backend trait. The orchestrator matches on
//! `cfg.transcription.backend` to pick one. Shared here: the HTTP client,
//! timeouts and the retry rule. See `agent_docs/transcription_backends.md`.

mod elevenlabs_batch;
pub(crate) mod keyterms;
mod voxtral_batch;
mod wav;

pub use elevenlabs_batch::{transcribe_batch, transcribe_medical_batch};
pub use voxtral_batch::transcribe_batch as transcribe_voxtral_batch;

use std::sync::OnceLock;
use std::time::Duration;

/// How long to wait for TCP + TLS to a backend's API host. A stalled connect
/// would strand the recording loop, which owns the hotkey receiver.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling on one batch request, end to end. Generous: it covers uploading a
/// whole recording and transcribing it, so it bounds a stall, not latency.
pub(crate) const BATCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Ceiling on one whole batch transcription including retries and (for
/// Voxtral) the vocab-correction call. Without it the per-request timeout
/// applies per call and a pathological sequence runs several times over.
pub(crate) const BATCH_OVERALL_TIMEOUT: Duration = Duration::from_secs(480);

/// Whether a batch attempt's HTTP status is worth retrying: rate limits (429)
/// and transient server failures are; anything else (auth, model, malformed
/// request) fails identically on retry.
pub(crate) fn batch_should_retry(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

#[cfg(test)]
mod batch_tests {
    use super::batch_should_retry;

    #[test]
    fn rate_limits_and_server_errors_retry() {
        assert!(batch_should_retry(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(batch_should_retry(reqwest::StatusCode::BAD_GATEWAY));
        assert!(batch_should_retry(reqwest::StatusCode::SERVICE_UNAVAILABLE));
        assert!(batch_should_retry(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
    }

    #[test]
    fn client_errors_do_not_retry() {
        assert!(!batch_should_retry(reqwest::StatusCode::BAD_REQUEST));
        assert!(!batch_should_retry(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!batch_should_retry(reqwest::StatusCode::NOT_FOUND));
        assert!(!batch_should_retry(reqwest::StatusCode::OK));
    }
}

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

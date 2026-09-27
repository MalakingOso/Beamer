//! The process-wide `reqwest` client, plus the `GET /v1/models` probe behind
//! Settings' Local AI card. Plain async on the dioxus-desktop runtime: never
//! wrap in `spawn_blocking`, which moves a future onto a thread that never
//! polls it.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Build the shared client with the configured connect timeout, fixed for the
/// process lifetime (a change applies on restart). Call once at startup before
/// [`http_client`]; later calls are no-ops. Takes a `Duration`, not the app
/// config, because `task_eval` path-includes this module without that type.
pub fn init_http_client(connect_timeout: Duration) {
    let _ = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .build()
            // A builder failure here means TLS init failed; the plain
            // constructor is no more likely to work, but falling back keeps a
            // request possible instead of panicking at startup.
            .unwrap_or_else(|_| reqwest::Client::new())
    });
}

/// The one connection pool for the process, used by `chat.rs` and by
/// `components::fetch` (hence `pub(crate)`: that module needs crate-rooted
/// paths, so it cannot live under `src/llm/`). Falls back to a default client
/// if [`init_http_client`] never ran, which only happens in tests and `task_eval`.
pub(crate) fn http_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(reqwest::Client::new)
}

/// One model the server is serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// GGUF filename stem; what a request's `model` field must say.
    pub id: String,
    /// Verbatim server status, displayed not branched on — a new server-side
    /// state should show rather than collapse into "unknown".
    pub status: String,
}

pub fn models_url(base_url: &str) -> String {
    format!("{}/v1/models", base_url.trim_end_matches('/'))
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    status: Option<ModelStatus>,
}

#[derive(Deserialize)]
struct ModelStatus {
    #[serde(default)]
    value: Option<String>,
}

pub fn parse_models(body: &str) -> Result<Vec<ModelInfo>> {
    let parsed: ModelsResponse =
        serde_json::from_str(body).context("model list was not the JSON we expected")?;
    Ok(parsed
        .data
        .into_iter()
        .map(|e| ModelInfo {
            id: e.id,
            status: e
                .status
                .and_then(|s| s.value)
                .unwrap_or_else(|| "unknown".to_string()),
        })
        .collect())
}

/// Turn a failed probe into user-facing text. Split from the request because
/// `reqwest::Error` cannot be built in tests.
pub fn failure_message(timed_out: bool, connect_failed: bool, status: Option<u16>) -> String {
    if let Some(code) = status {
        return format!("Server returned HTTP {code}");
    }
    if timed_out {
        return "No response — the server is reachable but did not answer".to_string();
    }
    if connect_failed {
        return "Server not running".to_string();
    }
    "Could not reach the server".to_string()
}

/// Ask the server what it is serving. Button press and settings-page open
/// only, never on a timer: a status read resets the per-model idle clock and
/// silently pins the extraction model in VRAM.
pub async fn probe(base_url: &str, timeout: Duration) -> Result<Vec<ModelInfo>, String> {
    let url = models_url(base_url);
    let response = http_client()
        .get(&url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| failure_message(e.is_timeout(), e.is_connect(), None))?;

    if !response.status().is_success() {
        return Err(failure_message(false, false, Some(response.status().as_u16())));
    }

    let body = response
        .text()
        .await
        .map_err(|e| failure_message(e.is_timeout(), false, None))?;

    parse_models(&body).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed real server response; the parser must ignore unknown fields.
    const REAL_RESPONSE: &str = r#"{
      "data": [
        {"id":"gemma-4-E2B_q4_0-it","object":"model","owned_by":"llamacpp",
         "status":{"value":"unloaded","args":["--host","127.0.0.1"]},
         "architecture":{"input_modalities":["text"]},"source":"models_dir"},
        {"id":"gemma-4-E4B_q4_0-it","object":"model",
         "status":{"value":"loaded","args":[]},
         "meta":{"n_ctx":8192,"size":5153960752}}
      ],
      "object": "list"
    }"#;

    #[test]
    fn parses_id_and_status_from_a_real_server_response() {
        let models = parse_models(REAL_RESPONSE).unwrap();
        assert_eq!(
            models,
            vec![
                ModelInfo { id: "gemma-4-E2B_q4_0-it".into(), status: "unloaded".into() },
                ModelInfo { id: "gemma-4-E4B_q4_0-it".into(), status: "loaded".into() },
            ]
        );
    }

    #[test]
    fn a_model_without_a_status_is_still_listed() {
        // Dropping this entry would hide a served model from the picker.
        let models = parse_models(r#"{"data":[{"id":"solo"}]}"#).unwrap();
        assert_eq!(models, vec![ModelInfo { id: "solo".into(), status: "unknown".into() }]);
    }

    #[test]
    fn an_empty_server_is_not_an_error() {
        assert!(parse_models(r#"{"data":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_models("not json at all").is_err());
        assert!(parse_models(r#"{"data":"nope"}"#).is_err());
    }

    #[test]
    fn models_url_tolerates_a_trailing_slash() {
        assert_eq!(models_url("http://127.0.0.1:8080"), "http://127.0.0.1:8080/v1/models");
        assert_eq!(models_url("http://127.0.0.1:8080/"), "http://127.0.0.1:8080/v1/models");
        assert_eq!(models_url("http://127.0.0.1:8080///"), "http://127.0.0.1:8080/v1/models");
    }

    #[test]
    fn init_http_client_is_idempotent() {
        // The backing OnceLock is process-global: this proves a second call
        // never panics, not which value won.
        init_http_client(Duration::from_millis(1_234));
        init_http_client(Duration::from_millis(9_999));
        let _ = http_client();
    }

    #[test]
    fn failures_are_described_in_the_users_terms() {
        assert_eq!(failure_message(false, true, None), "Server not running");
        assert!(failure_message(true, false, None).starts_with("No response"));
        assert_eq!(failure_message(false, false, Some(503)), "Server returned HTTP 503");
        // A status is the most specific thing known, so it wins over the flags.
        assert_eq!(failure_message(true, true, Some(500)), "Server returned HTTP 500");
    }
}

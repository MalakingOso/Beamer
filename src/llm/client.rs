//! HTTP client for the standalone llama.cpp server.
//!
//! Plain async `reqwest`, not `spawn_blocking`: `reqwest::Client` is already
//! async and its futures are driven by the tokio runtime dioxus-desktop owns.
//! Wrapping an async call in `spawn_blocking` would move a future onto a
//! blocking thread that never polls it.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Shared client, mirroring `transcription::http_client()`: one connection pool
/// for the process rather than a fresh one per probe.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// One model the server is serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// The GGUF filename stem. This is what a request's `model` field must say.
    pub id: String,
    /// `loaded`, `sleeping` or `unloaded`, verbatim from the server. Passed
    /// through rather than parsed into an enum: it is displayed, not branched
    /// on, and a server that grows a fourth state should show it rather than
    /// collapse it into "unknown".
    pub status: String,
}

/// `GET {base_url}/v1/models`, tolerating a trailing slash on the configured
/// URL — users paste URLs, and `http://host:8080//v1/models` is a 404.
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

/// Turn a failed probe into something worth showing a user.
///
/// Split from the request itself because a `reqwest::Error` cannot be
/// constructed in a test, and the classification is the part that can be wrong.
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

/// Ask the server what it is serving.
///
/// ⚠️ **Never call this on a timer, and never as a background health check.**
/// A status read resets the server's per-model idle clock, so a periodic probe
/// pins the ~3 GB extraction model in VRAM permanently — with no error, no log
/// line and no user-visible symptom until something else needs the memory. On
/// button press and once when the settings page opens, nowhere else. This is a
/// correctness constraint, not a performance preference.
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

    /// Trimmed from a real response of the running server (2026-08-21). The
    /// server sends far more per entry — args, presets, architecture — and the
    /// parser must ignore all of it rather than fail on an unknown field.
    const REAL_RESPONSE: &str = r#"{
      "data": [
        {"id":"gemma-4-E2B_q4_0-it","object":"model","owned_by":"llamacpp",
         "status":{"value":"unloaded","args":["--host","127.0.0.1"]},
         "architecture":{"input_modalities":["text"]},"source":"models_dir"},
        {"id":"s1-mini-q4_k_m","object":"model",
         "status":{"value":"loaded","args":[]},
         "meta":{"n_ctx":8192,"size":478268416}}
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
                ModelInfo { id: "s1-mini-q4_k_m".into(), status: "loaded".into() },
            ]
        );
    }

    #[test]
    fn a_model_without_a_status_is_still_listed() {
        // The picker is populated from this list. Dropping an entry because its
        // status was missing would hide a model the server is actually serving.
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
    fn failures_are_described_in_the_users_terms() {
        assert_eq!(failure_message(false, true, None), "Server not running");
        assert!(failure_message(true, false, None).starts_with("No response"));
        assert_eq!(failure_message(false, false, Some(503)), "Server returned HTTP 503");
        // A status is the most specific thing known, so it wins over the flags.
        assert_eq!(failure_message(true, true, Some(500)), "Server returned HTTP 500");
    }
}

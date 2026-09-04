//! Local model client config. Beamer is a plain HTTP client of the standalone
//! server (see `deploy/llama-beamer.service`); the connection surface is just
//! a base URL. Launch settings live in the unit file, per-model settings in
//! `deploy/llama-models.ini`.

pub mod chat;
pub mod cleanup;
pub mod client;
pub mod extract;
pub mod prompts;

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Floor on `connect_timeout_ms`: `0` would fail every request instantly, so
/// [`LlmConfig::connect_timeout`] clamps here — the UI `min` cannot cover a
/// hand-edited `config.toml`. 100ms is a floor nobody mistakes for "off".
pub const MIN_CONNECT_TIMEOUT_MS: u64 = 100;

/// Required attribution for the cleanup model: Apache 2.0 plus a binding term
/// demanding exactly `"S1-mini" by "Superwhisper"`. Pinned by test — do not
/// "fix" the nested quoting.
pub const MODEL_CREDIT: &str = r#""S1-mini" by "Superwhisper""#;

/// Connection settings for the local model server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Deliberately generous: waking a sleeping model costs seconds. A timeout
    /// skips cleanup, never loses the note.
    #[serde(default = "default_timeout_ms")]
    pub request_timeout_ms: u64,
    /// TCP-open timeout, separate from `request_timeout_ms`, so an asleep host
    /// fails in seconds. Baked into the shared client at startup; changing it
    /// takes effect on restart, not immediately.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub cleanup: CleanupConfig,
    #[serde(default)]
    pub extract: ExtractConfig,
}

/// Stage 1 — transcript cleanup. See [`MODEL_CREDIT`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanupConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// A server-side model name, not a path — it must match an id from
    /// `GET /v1/models`, which is the GGUF filename stem.
    #[serde(default = "default_cleanup_model")]
    pub model: String,
    /// casual | semi-casual | semi-formal | formal
    #[serde(default = "default_styling")]
    pub styling: String,
    /// prose | lists
    #[serde(default = "default_structure")]
    pub structure: String,
    /// general | email
    #[serde(default = "default_context")]
    pub context: String,
}

/// Stage 2 — task extraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_extract_model")]
    pub model: String,
    /// Below this, a suggestion is not shown at all. Extraction is a precision
    /// problem: a wrong task costs more than a missed one.
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,
}

fn default_true() -> bool { true }
fn default_base_url() -> String { "http://127.0.0.1:8080".into() }
fn default_timeout_ms() -> u64 { 15_000 }
fn default_connect_timeout_ms() -> u64 { 5_000 }
fn default_cleanup_model() -> String { "s1-mini-q4_k_m".into() }
fn default_extract_model() -> String { "gemma-4-E4B_q4_0-it".into() }
fn default_styling() -> String { "semi-formal".into() }
fn default_structure() -> String { "lists".into() }
fn default_context() -> String { "general".into() }
fn default_min_confidence() -> f32 { 0.5 }

impl LlmConfig {
    /// `connect_timeout_ms` as a `Duration`, clamped to
    /// [`MIN_CONNECT_TIMEOUT_MS`]. `#[serde(default)]` cannot catch an explicit
    /// `0`, so the clamp lives here, where the value becomes a `Duration`.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms.max(MIN_CONNECT_TIMEOUT_MS))
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            base_url: default_base_url(),
            request_timeout_ms: default_timeout_ms(),
            connect_timeout_ms: default_connect_timeout_ms(),
            cleanup: CleanupConfig::default(),
            extract: ExtractConfig::default(),
        }
    }
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            model: default_cleanup_model(),
            styling: default_styling(),
            structure: default_structure(),
            context: default_context(),
        }
    }
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            model: default_extract_model(),
            min_confidence: default_min_confidence(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_credit_is_exactly_what_the_licence_requires() {
        // Longhand on purpose: must fail if the string is ever reformatted.
        let required = "\"S1-mini\" by \"Superwhisper\"";
        assert_eq!(
            MODEL_CREDIT, required,
            "s1-mini's licence binds Beamer to this exact capitalization and quoting"
        );
    }

    #[test]
    fn defaults_point_at_a_local_server_with_a_generous_timeout() {
        let cfg = LlmConfig::default();
        assert_eq!(cfg.base_url, "http://127.0.0.1:8080");
        assert!(
            cfg.request_timeout_ms >= 15_000,
            "waking a sleeping extraction model costs ~1.7s, or ~4s from cold; \
             a short timeout turns a slow answer into no answer"
        );
        assert!(!cfg.base_url.ends_with('/'), "the default must not need normalizing");
    }

    #[test]
    fn connect_timeout_defaults_short_enough_that_asleep_fails_fast() {
        let cfg = LlmConfig::default();
        assert_eq!(cfg.connect_timeout_ms, 5_000);
        assert!(
            cfg.connect_timeout_ms < cfg.request_timeout_ms,
            "connect is the fast fail path; it must stay well under the \
             generous total timeout or it buys nothing over a tailnet"
        );
    }

    #[test]
    fn an_old_config_missing_connect_timeout_ms_still_loads() {
        // Old configs lack this field, so it must stay `#[serde(default)]`.
        let toml = r#"
            enabled = true
            base_url = "http://127.0.0.1:8080"
            request_timeout_ms = 15000
        "#;
        let cfg: LlmConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.connect_timeout_ms, 5_000);
    }

    #[test]
    fn stage_models_default_to_the_files_the_deploy_preset_serves() {
        let cfg = LlmConfig::default();
        assert_eq!(cfg.cleanup.model, "s1-mini-q4_k_m");
        assert_eq!(cfg.extract.model, "gemma-4-E4B_q4_0-it");
    }
}

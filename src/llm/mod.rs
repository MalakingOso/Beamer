//! On-device model client and its configuration.
//!
//! Beamer does **not** spawn or manage the model server. It runs standalone —
//! see `deploy/llama-beamer.service` — and Beamer is a plain HTTP client, so
//! the entire connection surface is a base URL. Launch settings live in the
//! unit file; per-model settings, including idle shutdown, live in
//! `deploy/llama-models.ini`.

pub mod chat;
pub mod cleanup;
pub mod client;
pub mod extract;
pub mod prompts;

use serde::{Deserialize, Serialize};

/// Required attribution for the cleanup model.
///
/// `superwhisper/s1-mini` is Apache 2.0 **plus a binding additional term**: any
/// use, distribution or integration must continue to identify the model as
/// `"S1-mini" by "Superwhisper"`, using that exact capitalization, regardless
/// of what the surrounding product is called.
///
/// It lives here, beside the config it licenses, and is pinned by an
/// exact-equality test. The failure mode this guards against is not malice but
/// tidiness: a later pass that "fixes" the nested quoting would put Beamer out
/// of compliance with no error and no symptom.
pub const MODEL_CREDIT: &str = r#""S1-mini" by "Superwhisper""#;

/// Connection settings for the local model server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Was the only connection setting before `connect_timeout_ms` joined it.
    /// Everything else about the server is the server's own business.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Deliberately generous. The extraction model can be asleep and waking it
    /// costs ~1.7s on top of the request, or ~4s if the whole server is cold.
    /// A timeout here means "the note was not cleaned", never "the note was
    /// lost", so erring long costs nothing and erring short costs cleanup.
    #[serde(default = "default_timeout_ms")]
    pub request_timeout_ms: u64,
    /// How long to wait for TCP (+ TLS, over a tailnet) to open, separate from
    /// `request_timeout_ms`. A desktop that is asleep on a tailnet should fail
    /// in seconds, not hang for the whole generous request timeout on every
    /// single text run.
    ///
    /// Measured RTT to a laptop over Tailscale was 13-289ms (mdev 109, WiFi
    /// power saving), so 5s leaves real margin without turning "asleep" into a
    /// multi-second stall.
    ///
    /// Baked into the shared client's `OnceLock` at process start
    /// (`llm::client::init_http_client`, called from `main.rs`) rather than
    /// read per-request, so changing it in Settings takes effect on the next
    /// restart, not immediately.
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
        // Written out longhand rather than re-derived from the constant: the
        // point is to fail loudly if the string is ever reformatted, and a test
        // that rebuilds it the same way the constant does would not.
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
        // Mirrors the pre-existing shape of config.toml before this field
        // existed — the field must be `#[serde(default)]`, not required.
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

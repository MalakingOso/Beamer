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
///
/// Off by default and toggled on its own, independently of `LlmConfig::enabled`:
/// the only model `crate::model_setup` installs is the extraction model, so a
/// server that has never been told about s1-mini answers every cleanup request
/// with an error, and the note footer reports a failure for a pass the user
/// never asked for. Extraction is the pass that earns its place on a sticky.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanupConfig {
    #[serde(default)]
    pub enabled: bool,
    /// A server-side model name, not a path — it must match an id from
    /// `GET /v1/models`, which is the GGUF filename stem.
    #[serde(default = "default_cleanup_model")]
    pub model: String,
    /// Overrides `LlmConfig::base_url` for this stage only. `None` (the
    /// common case) means "same server as everything else" — see
    /// [`LlmConfig::cleanup_base_url`].
    #[serde(default)]
    pub base_url: Option<String>,
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
    /// Overrides `LlmConfig::base_url` for this stage only — see
    /// [`LlmConfig::extract_base_url`]. What makes running extraction on a
    /// different host than cleanup possible (e.g. a local CPU-only server for
    /// extraction while cleanup stays on a remote GPU box).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Below this, a suggestion is not shown at all. Extraction is a precision
    /// problem: a wrong task costs more than a missed one.
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,
}

fn default_true() -> bool { true }
fn default_base_url() -> String { "http://127.0.0.1:8080".into() }
fn default_timeout_ms() -> u64 { 60_000 }
fn default_connect_timeout_ms() -> u64 { 5_000 }
fn default_cleanup_model() -> String { "s1-mini-q4_k_m".into() }

/// Only a bundled aarch64 Windows build ever has a server that can serve
/// K2-Horizon (its llama.cpp fork build exists for that arch only —
/// `crate::model_setup` downloads the model and starts it there). Every
/// other target — x86_64 Windows, Linux/callisto — has upstream llama.cpp,
/// which cannot load `K2HorizonForCausalLM` at all, so a fresh config there
/// must default back to what it always defaulted to.
#[cfg(target_arch = "aarch64")]
fn default_extract_model() -> String { "K2-Horizon-0.9B-Q8_0".into() }
#[cfg(not(target_arch = "aarch64"))]
fn default_extract_model() -> String { "gemma-4-E4B_q4_0-it".into() }

fn default_styling() -> String { "semi-formal".into() }
fn default_structure() -> String { "lists".into() }
fn default_context() -> String { "general".into() }
fn default_min_confidence() -> f32 { 0.5 }

/// Shared by `cleanup_base_url`/`extract_base_url`: an override that is
/// absent or blank falls back to `shared`.
fn stage_base_url<'a>(override_url: &'a Option<String>, shared: &'a str) -> &'a str {
    match override_url.as_deref() {
        Some(url) if !url.trim().is_empty() => url,
        _ => shared,
    }
}

impl LlmConfig {
    /// `connect_timeout_ms` as a `Duration`, clamped to
    /// [`MIN_CONNECT_TIMEOUT_MS`]. `#[serde(default)]` cannot catch an explicit
    /// `0`, so the clamp lives here, where the value becomes a `Duration`.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms.max(MIN_CONNECT_TIMEOUT_MS))
    }

    /// The server to send cleanup requests to: `cleanup.base_url` if set and
    /// non-blank, otherwise the shared `base_url`. Kept as a fallback (not
    /// required on every stage) so an existing single-`base_url` config still
    /// routes both stages the way it always has. A blank override (an empty
    /// string, e.g. a hand-edited `base_url = ""`) is treated the same as
    /// absent rather than as a literal empty host.
    pub fn cleanup_base_url(&self) -> &str {
        stage_base_url(&self.cleanup.base_url, &self.base_url)
    }

    /// The server to send extraction requests to. See [`Self::cleanup_base_url`].
    pub fn extract_base_url(&self) -> &str {
        stage_base_url(&self.extract.base_url, &self.base_url)
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
            enabled: false,
            model: default_cleanup_model(),
            base_url: None,
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
            base_url: None,
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
            cfg.request_timeout_ms >= 45_000,
            "a 14-note CPU batch of the default extraction model measured \
             1.3-12.4s/note at its configured reasoning effort, before any \
             cold-load reload on top; 15s is the tail, not headroom"
        );
        assert!(!cfg.base_url.ends_with('/'), "the default must not need normalizing");
    }

    #[test]
    fn cleanup_is_off_until_the_user_asks_for_it() {
        let cfg = LlmConfig::default();
        assert!(cfg.enabled, "extraction is the pass a fresh install can actually run");
        assert!(
            !cfg.cleanup.enabled,
            "model_setup installs the extraction model and nothing else, so a default-on              cleanup pass reports a failure on every note for a server that was never              asked to serve s1-mini"
        );
        assert!(cfg.extract.enabled, "the two stages are toggled independently");
    }

    #[test]
    fn an_old_config_that_turned_cleanup_on_keeps_it_on() {
        // The default flipped; an explicit `true` on disk must still win, or
        // the change silently disables a pass someone is relying on.
        let cfg: LlmConfig = toml::from_str("[cleanup]\nenabled = true\n").unwrap();
        assert!(cfg.cleanup.enabled);
    }

    #[test]
    fn a_stage_with_no_base_url_override_falls_back_to_the_shared_one() {
        let mut cfg = LlmConfig::default();
        cfg.base_url = "https://callisto.example.ts.net".into();
        assert_eq!(cfg.cleanup_base_url(), "https://callisto.example.ts.net");
        assert_eq!(cfg.extract_base_url(), "https://callisto.example.ts.net");
    }

    #[test]
    fn a_stage_base_url_override_wins_over_the_shared_one() {
        let mut cfg = LlmConfig::default();
        cfg.base_url = "https://callisto.example.ts.net".into();
        cfg.extract.base_url = Some("http://127.0.0.1:8080".into());
        assert_eq!(cfg.cleanup_base_url(), "https://callisto.example.ts.net");
        assert_eq!(cfg.extract_base_url(), "http://127.0.0.1:8080");
    }

    #[test]
    fn an_empty_stage_base_url_falls_back_to_the_shared_one() {
        // A hand-edited `base_url = ""` must not become a literal empty host.
        let mut cfg = LlmConfig::default();
        cfg.base_url = "https://callisto.example.ts.net".into();
        cfg.extract.base_url = Some("".into());
        assert_eq!(cfg.extract_base_url(), "https://callisto.example.ts.net");
    }

    #[test]
    fn an_old_config_with_only_a_shared_base_url_still_routes_both_stages_there() {
        // Predates the per-stage override: must not silently drop either
        // stage to the localhost default.
        let toml = r#"
            base_url = "https://callisto.example.ts.net"
        "#;
        let cfg: LlmConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.cleanup_base_url(), "https://callisto.example.ts.net");
        assert_eq!(cfg.extract_base_url(), "https://callisto.example.ts.net");
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
        // Only a bundled aarch64 build ever has a server that can serve
        // K2-Horizon (see `default_extract_model`'s doc comment); every
        // other target defaults back to Gemma.
        #[cfg(target_arch = "aarch64")]
        assert_eq!(cfg.extract.model, "K2-Horizon-0.9B-Q8_0");
        #[cfg(not(target_arch = "aarch64"))]
        assert_eq!(cfg.extract.model, "gemma-4-E4B_q4_0-it");
    }
}

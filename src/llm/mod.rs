//! Local model client config. Beamer is a plain HTTP client of the standalone
//! server (see `deploy/llama-beamer.service`); the connection surface is just
//! a base URL. Launch settings live in the unit file, per-model settings in
//! `deploy/llama-models.ini`.

pub mod chat;
pub mod client;
pub mod extract;
pub mod prompts;

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Floor on `connect_timeout_ms`: `0` would fail every request instantly, so
/// [`LlmConfig::connect_timeout`] clamps here — the UI `min` cannot cover a
/// hand-edited `config.toml`. 100ms is a floor nobody mistakes for "off".
pub const MIN_CONNECT_TIMEOUT_MS: u64 = 100;

/// Connection settings for the local model server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Deliberately generous: waking a sleeping model costs seconds. A timeout
    /// skips the pass, never loses the note.
    #[serde(default = "default_timeout_ms")]
    pub request_timeout_ms: u64,
    /// TCP-open timeout, separate from `request_timeout_ms`, so an asleep host
    /// fails in seconds. Baked into the shared client at startup; changing it
    /// takes effect on restart, not immediately.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub extract: ExtractConfig,
}

/// Task extraction: the only model pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_extract_model")]
    pub model: String,
    /// Overrides `LlmConfig::base_url` for extraction — see
    /// [`LlmConfig::extract_base_url`]. Allows pointing extraction at a
    /// different host than the shared default (e.g. a local CPU-only server
    /// while the shared URL points at a remote GPU box).
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

fn default_min_confidence() -> f32 { 0.5 }

impl LlmConfig {
    /// `connect_timeout_ms` as a `Duration`, clamped to
    /// [`MIN_CONNECT_TIMEOUT_MS`]. `#[serde(default)]` cannot catch an explicit
    /// `0`, so the clamp lives here, where the value becomes a `Duration`.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms.max(MIN_CONNECT_TIMEOUT_MS))
    }

    /// The server to send extraction requests to: `extract.base_url` if set
    /// and non-blank, otherwise the shared `base_url`. Kept as a fallback so
    /// an existing single-`base_url` config still routes the way it always
    /// has. A blank override (an empty string, e.g. a hand-edited `base_url =
    /// ""`) is treated the same as absent rather than as a literal empty host.
    pub fn extract_base_url(&self) -> &str {
        match self.extract.base_url.as_deref() {
            Some(url) if !url.trim().is_empty() => url,
            _ => &self.base_url,
        }
    }

    /// Whether the extraction pass is wanted right now. Both switches, because
    /// `enabled` is a master gate over `extract.enabled`, not an alternative
    /// to it — a caller that checks only one of them reports the wrong answer
    /// for half the combinations. Lives here rather than at the call sites so
    /// the pipeline's mid-flight re-check cannot drift from the footer.
    pub fn extract_wanted(&self) -> bool {
        self.enabled && self.extract.enabled
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            base_url: default_base_url(),
            request_timeout_ms: default_timeout_ms(),
            connect_timeout_ms: default_connect_timeout_ms(),
            extract: ExtractConfig::default(),
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
    fn extraction_is_on_for_a_fresh_install() {
        let cfg = LlmConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.extract.enabled, "the pass a fresh install can actually run");
    }

    #[test]
    fn the_master_switch_gates_the_pass_rather_than_replacing_it() {
        let mut cfg = LlmConfig::default();
        assert!(cfg.extract_wanted());

        // Master off: the pass does not run, whatever its own switch says. A
        // caller reading only `extract.enabled` here would run a pass the
        // user has turned the whole feature off for.
        cfg.enabled = false;
        assert!(!cfg.extract_wanted());

        // Master on, pass off: stays off. The switches are independent.
        cfg.enabled = true;
        cfg.extract.enabled = false;
        assert!(!cfg.extract_wanted());
    }

    #[test]
    fn extraction_with_no_base_url_override_falls_back_to_the_shared_one() {
        let mut cfg = LlmConfig::default();
        cfg.base_url = "https://callisto.example.ts.net".into();
        assert_eq!(cfg.extract_base_url(), "https://callisto.example.ts.net");
    }

    #[test]
    fn an_extract_base_url_override_wins_over_the_shared_one() {
        let mut cfg = LlmConfig::default();
        cfg.base_url = "https://callisto.example.ts.net".into();
        cfg.extract.base_url = Some("http://127.0.0.1:8080".into());
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
    fn an_old_config_with_only_a_shared_base_url_still_routes_there() {
        // Predates the per-stage override: must not silently drop
        // extraction to the localhost default.
        let toml = r#"
            base_url = "https://callisto.example.ts.net"
        "#;
        let cfg: LlmConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.extract_base_url(), "https://callisto.example.ts.net");
    }

    #[test]
    fn a_pre_strip_config_with_a_cleanup_table_still_loads() {
        // The cleanup pass is gone, but its table sits in existing
        // config.toml files. Unknown keys are ignored, so those files
        // load and the stale table drops out on the next save.
        let toml = r#"
            base_url = "https://callisto.example.ts.net"
            [cleanup]
            enabled = true
        "#;
        let cfg: LlmConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.extract_base_url(), "https://callisto.example.ts.net");
        assert!(cfg.extract.enabled);
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
    fn the_extraction_model_defaults_to_the_file_the_deploy_preset_serves() {
        let cfg = LlmConfig::default();
        // Only a bundled aarch64 build ever has a server that can serve
        // K2-Horizon (see `default_extract_model`'s doc comment); every
        // other target defaults back to Gemma.
        #[cfg(target_arch = "aarch64")]
        assert_eq!(cfg.extract.model, "K2-Horizon-0.9B-Q8_0");
        #[cfg(not(target_arch = "aarch64"))]
        assert_eq!(cfg.extract.model, "gemma-4-E4B_q4_0-it");
    }
}

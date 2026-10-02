//! User settings: `config.toml` (serde structs with per-field defaults, load
//! migrations, atomic save), loaded at startup and edited by the settings UI.
//! API keys are the exception: they live in the OS keyring, never on disk.
//! `vocabulary` holds the custom STT terms. Schema: `agent_docs/config_schema.md`.

mod keys;
mod migrate;
pub mod vocabulary;

pub use keys::{load_api_key, save_api_key};
use migrate::{migrate_injection_backends, migrate_transcription_backend};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level config, serialized as TOML. Missing fields use serde defaults;
/// the file is created on first launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub recording: RecordingConfig,
    #[serde(default)]
    pub transcription: TranscriptionConfig,
    #[serde(default)]
    pub injection: InjectionConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub notes: NotesConfig,
    #[serde(default)]
    pub llm: crate::llm::LlmConfig,
    #[serde(default)]
    pub sync: SyncConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub pause_media: bool,
    /// Chord dictating into a sticky note instead of injecting.
    /// Empty means unconfigured: no note binding is registered.
    #[serde(default)]
    pub note_hotkey: String,
    /// "toggle" (default) or "hold". Notes run long; holding a chord throughout is awkward.
    #[serde(default = "default_note_mode")]
    pub note_mode: String,
    /// Chord logging a spoken line to the Done list. Empty means off.
    #[serde(default)]
    pub done_hotkey: String,
    /// "hold" (default): an accomplishment is one short sentence.
    #[serde(default = "default_done_mode")]
    pub done_mode: String,
}

impl RecordingConfig {
    /// Parsed note-capture binding, or `None` when unconfigured or unparseable.
    /// A bad value yields `None` rather than a fallback chord the user never chose.
    pub fn note_hotkey_config(&self) -> Option<crate::hotkey::HotkeyConfig> {
        if self.note_hotkey.trim().is_empty() {
            return None;
        }
        crate::hotkey::HotkeyConfig::parse(&self.note_hotkey, self.note_mode == "toggle")
    }

    /// Parsed Done-list binding; same rules as [`Self::note_hotkey_config`].
    pub fn done_hotkey_config(&self) -> Option<crate::hotkey::HotkeyConfig> {
        if self.done_hotkey.trim().is_empty() {
            return None;
        }
        crate::hotkey::HotkeyConfig::parse(&self.done_hotkey, self.done_mode == "toggle")
    }
}

/// Sticky note behaviour and appearance defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotesConfig {
    /// Mutter only: `stick()` note windows so they follow across workspaces.
    #[serde(default = "default_true")]
    pub all_workspaces: bool,
    #[serde(default = "default_note_color")]
    pub default_color: String,
}

impl Default for NotesConfig {
    fn default() -> Self {
        Self { all_workspaces: true, default_color: default_note_color() }
    }
}

/// Live sync against `sync_server` (see `agent_docs/sync.md`).
/// Empty `url` means off — a network-reaching feature must never default on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// `wss://<tailnet-host>/sync`, or empty. Per machine, never synced.
    #[serde(default)]
    pub url: String,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self { url: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_language")]
    pub language: String,
    /// Ask ElevenLabs to drop fillers and false starts (others ignore it).
    /// Off by default: it changes what was said, not just spelling.
    #[serde(default)]
    pub no_verbatim: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionConfig {
    /// Ordered injection fallback chain; the default is per-OS.
    #[serde(default = "default_backends")]
    pub backends: Vec<String>,
    #[serde(default)]
    pub debug_logging: bool,
    /// Linux only: paste chord for the clipboard backend.
    /// "auto" (default, per-app via focus helper, else Ctrl+Shift+V),
    /// "ctrl_v", or "ctrl_shift_v". Env `BEAMER_PASTE_SHORTCUT` overrides.
    #[serde(default = "default_paste_shortcut")]
    pub paste_shortcut: String,
    /// Legacy single-backend field: migrated into `backends` on load, never written back.
    #[serde(default, skip_serializing)]
    preferred_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    #[serde(default = "default_true")]
    pub pill_enabled: bool,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
}

fn default_hotkey() -> String { "Ctrl+Space".into() }
fn default_mode() -> String { "hold".into() }
fn default_backend() -> String { "elevenlabs_batch".into() }
fn default_language() -> String { "en".into() }
fn default_backends() -> Vec<String> { crate::injection::default_backend_names() }
fn default_paste_shortcut() -> String { "auto".into() }
fn default_note_mode() -> String { "toggle".into() }
fn default_done_mode() -> String { "hold".into() }
fn default_note_color() -> String { "random".into() }
fn default_true() -> bool { true }


impl Default for Config {
    fn default() -> Self {
        Self {
            recording: RecordingConfig::default(),
            transcription: TranscriptionConfig::default(),
            injection: InjectionConfig::default(),
            appearance: AppearanceConfig::default(),
            notes: NotesConfig::default(),
            llm: crate::llm::LlmConfig::default(),
            sync: SyncConfig::default(),
        }
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            mode: default_mode(),
            pause_media: false,
            note_hotkey: String::new(),
            note_mode: default_note_mode(),
            done_hotkey: String::new(),
            done_mode: default_done_mode(),
        }
    }
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            language: default_language(),
            no_verbatim: false,
        }
    }
}

impl Default for InjectionConfig {
    fn default() -> Self {
        Self {
            backends: default_backends(),
            debug_logging: false,
            paste_shortcut: default_paste_shortcut(),
            preferred_method: None,
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            pill_enabled: default_true(),
            auto_start: false,
            auto_check_updates: default_true(),
        }
    }
}

impl Config {
    /// `%APPDATA%\Beamer` / `~/.config/Beamer`. If the OS can't name a config
    /// directory, fall back to the temp dir rather than crash on launch.
    pub fn config_dir() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| {
            tracing::error!("Could not determine the config directory; using the temp dir");
            std::env::temp_dir()
        });
        base.join("Beamer")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Load `config.toml`, writing defaults on first run and re-saving migrations.
    /// `try_exists`, not `exists`: a stat failure must not pass for a fresh
    /// install that saves defaults over real settings. Bad TOML is moved to
    /// `config.toml.corrupt` first, so the original is never overwritten.
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        match path.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                let config = Config::default();
                config.save()?;
                return Ok(config);
            }
            Err(e) => {
                anyhow::bail!("Could not tell whether {} exists: {e}", path.display());
            }
        }

        let contents = std::fs::read_to_string(&path)?;
        let mut config: Config = match toml::from_str(&contents) {
            Ok(config) => config,
            Err(e) => {
                let backup = path.with_extension("toml.corrupt");
                tracing::error!(
                    "{} is not valid TOML ({e}); preserving it as {} and starting fresh",
                    path.display(),
                    backup.display()
                );
                if let Err(e) = std::fs::rename(&path, &backup) {
                    tracing::error!("Could not preserve corrupt config: {e}");
                }
                let config = Config::default();
                config.save()?;
                return Ok(config);
            }
        };
        let mut dirty = false;

        // Migrate old preferred_method → backends list
        if let Some(ref method) = config.injection.preferred_method {
            if config.injection.backends == default_backends() {
                config.injection.backends = match method.as_str() {
                    "auto" => default_backends(),
                    other => vec![other.to_string()],
                };
            }
            config.injection.preferred_method = None;
            dirty = true;
        }

        if migrate_injection_backends(&mut config.injection.backends) {
            dirty = true;
        }

        if migrate_transcription_backend(&mut config.transcription.backend) {
            dirty = true;
        }

        if dirty {
            if let Err(e) = config.save() {
                tracing::warn!("Could not save migrated config (will retry next launch): {e}");
            }
        }

        Ok(config)
    }

    /// Persist atomically (temp file + rename) so a mid-write crash can't
    /// leave a truncated `config.toml`.
    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let contents = toml::to_string_pretty(self)?;
        let path = Self::config_path();
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, contents)?;
        if let Err(e) = crate::notes::sync_doc::rename_with_retry(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod note_config_tests {
    use super::*;

    #[test]
    fn note_hotkey_is_unset_by_default() {
        let cfg = RecordingConfig::default();
        assert_eq!(cfg.note_hotkey, "", "no default chord may be stolen from another app");
        assert!(
            cfg.note_hotkey_config().is_none(),
            "an unset note hotkey must produce no binding at all"
        );
    }

    #[test]
    fn unparseable_note_hotkey_yields_no_binding_rather_than_a_wrong_one() {
        let cfg = RecordingConfig {
            note_hotkey: "Ctrl+NotAKey".into(),
            ..RecordingConfig::default()
        };
        assert!(cfg.note_hotkey_config().is_none());
    }

    #[test]
    fn sync_url_is_unset_by_default() {
        let cfg = SyncConfig::default();
        assert_eq!(cfg.url, "", "sync must not dial out unless a machine was told a server exists");
    }

    #[test]
    fn an_old_config_missing_sync_still_loads() {
        // Mirrors a config.toml written before this field existed.
        let toml = r#"
            [recording]
            hotkey = "Ctrl+Space"
            mode = "hold"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.sync.url, "");
    }
}

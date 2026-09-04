pub mod vocabulary;

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
    /// Drop fillers/false starts rather than transcribing literally.
    /// ElevenLabs only; other backends ignore it. Off by default — it changes
    /// what was said, not just spelling.
    #[serde(default)]
    pub no_verbatim: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionConfig {
    /// Ordered list of injection backends to try (e.g. ["ydotool", "clipboard"]).
    #[serde(default = "default_backends")]
    pub backends: Vec<String>,
    #[serde(default)]
    pub debug_logging: bool,
    /// Linux only: paste chord for the clipboard backend.
    /// "auto" (default, per-app via focus helper, else Ctrl+Shift+V),
    /// "ctrl_v", or "ctrl_shift_v". Env `BEAMER_PASTE_SHORTCUT` overrides.
    #[serde(default = "default_paste_shortcut")]
    pub paste_shortcut: String,
    /// Migration: old field from previous config format. Read but never written back.
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
fn default_backend() -> String { "elevenlabs".into() }
fn default_language() -> String { "en".into() }
fn default_backends() -> Vec<String> { crate::injection::default_backend_names() }
fn default_paste_shortcut() -> String { "auto".into() }
fn default_note_mode() -> String { "toggle".into() }
fn default_note_color() -> String { "purple".into() }
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
    pub fn config_dir() -> PathBuf {
        let base = dirs::config_dir().expect("Could not determine config directory");
        base.join("Beamer")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// `try_exists`, not `exists`: a stat failure must not look like a fresh
    /// install, or defaults would be saved over the user's real settings.
    /// Unparseable TOML is quarantined to `*.corrupt` (reloadable default),
    /// never silently discarded — callers fall back to default on `Err` and
    /// would otherwise save that default over the recoverable original.
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

        if dirty {
            let _ = config.save();
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

/// Normalize a stored injection backend chain against the current build.
/// Returns `true` if the list changed (caller should re-save).
fn migrate_injection_backends(backends: &mut Vec<String>) -> bool {
        let mut dirty = false;

        // Drop backends removed from this build (wtype is current again —
        // first-class on wlroots behind a fast-failing GNOME/KDE probe).
        const REMOVED: &[&str] = &["dotool", "enigo", "atspi"];
        let before = backends.len();
        backends.retain(|b| !REMOVED.contains(&b.as_str()));
        if backends.len() != before {
            dirty = true;
        }

        // Legacy ["ydotool", "clipboard"] default upgrades so existing users
        // pick up gnome/wtype. Custom orderings are left alone.
        if backends.as_slice() == ["ydotool".to_string(), "clipboard".to_string()] {
            *backends = default_backends();
            dirty = true;
        }

        // Safety net: never leave the user with an empty backend chain.
        if backends.is_empty() {
            *backends = default_backends();
            dirty = true;
        }

    dirty
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    fn chain(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn legacy_default_upgrades_to_new_default() {
        let mut backends = chain(&["ydotool", "clipboard"]);
        assert!(migrate_injection_backends(&mut backends));
        assert_eq!(backends, default_backends());
    }

    #[test]
    fn custom_chain_is_untouched() {
        let mut backends = chain(&["clipboard", "ydotool"]);
        assert!(!migrate_injection_backends(&mut backends));
        assert_eq!(backends, chain(&["clipboard", "ydotool"]));
    }

    #[test]
    fn wtype_is_no_longer_stripped() {
        let mut backends = chain(&["wtype", "clipboard"]);
        assert!(!migrate_injection_backends(&mut backends));
        assert_eq!(backends, chain(&["wtype", "clipboard"]));
    }

    #[test]
    fn dead_backends_are_stripped() {
        let mut backends = chain(&["dotool", "enigo", "clipboard"]);
        assert!(migrate_injection_backends(&mut backends));
        assert_eq!(backends, chain(&["clipboard"]));
    }

    #[test]
    fn empty_chain_falls_back_to_default() {
        let mut backends = chain(&["atspi"]);
        assert!(migrate_injection_backends(&mut backends));
        assert_eq!(backends, default_backends());
    }
}

/// In-memory cache of API keys confirmed by the OS keyring, by credential name.
/// Misses/errors are never cached, so a transiently locked keyring at startup
/// can't permanently mask a key that's actually present.
static KEY_CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Read an API key from the OS keyring (service "beamer"), cached after the
/// first successful read. Misses/errors retry the keyring on the next call.
pub fn load_api_key(name: &str) -> String {
    if let Some(cached) = KEY_CACHE.lock().unwrap().get(name) {
        return cached.clone();
    }

    match keyring::Entry::new("beamer", name).and_then(|e| e.get_password()) {
        Ok(password) => {
            KEY_CACHE
                .lock()
                .unwrap()
                .insert(name.to_string(), password.clone());
            password
        }
        Err(_) => String::new(),
    }
}

/// Write (or delete, when `value` is empty) an API key in the OS keyring.
/// The cache updates only after the keyring confirms, so it never claims
/// state that isn't durably stored.
pub fn save_api_key(name: &str, value: &str) {
    if value.is_empty() {
        let result = keyring::Entry::new("beamer", name).and_then(|e| e.delete_credential());
        match result {
            // Deleted or already absent — either way, drop the cached value.
            Ok(()) | Err(keyring::Error::NoEntry) => {
                KEY_CACHE.lock().unwrap().remove(name);
            }
            Err(_) => {}
        }
    } else if keyring::Entry::new("beamer", name)
        .and_then(|e| e.set_password(value))
        .is_ok()
    {
        KEY_CACHE
            .lock()
            .unwrap()
            .insert(name.to_string(), value.to_string());
    }
}

#[cfg(test)]
mod key_cache_tests {
    use super::*;

    /// Cache-only check: must serve the cached value without touching the
    /// real keyring backend (fails/hangs headless).
    #[test]
    fn load_api_key_serves_from_cache_without_touching_keyring() {
        let name = "tb12_test_cache_only_key_never_written_to_real_keyring";
        KEY_CACHE
            .lock()
            .unwrap()
            .insert(name.to_string(), "cached-value".to_string());

        assert_eq!(load_api_key(name), "cached-value");

        // Clean up so this test doesn't leak state into others.
        KEY_CACHE.lock().unwrap().remove(name);
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
    fn configured_note_hotkey_parses_to_a_binding() {
        let cfg = RecordingConfig {
            note_hotkey: "Ctrl+Shift+N".into(),
            note_mode: "toggle".into(),
            ..RecordingConfig::default()
        };
        let parsed = cfg.note_hotkey_config().expect("should parse");
        assert!(parsed.ctrl);
        assert!(parsed.shift);
        assert!(!parsed.alt);
        assert_eq!(parsed.trigger_vk, 0x4E); // N
        assert!(parsed.is_toggle, "note capture defaults to toggle, not hold");
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
    fn notes_config_defaults() {
        let cfg = NotesConfig::default();
        assert!(cfg.all_workspaces, "a sticky note should follow you across workspaces");
        assert_eq!(cfg.default_color, "purple");
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

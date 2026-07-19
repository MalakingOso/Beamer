pub mod vocabulary;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level application configuration. Serialized as TOML to `%APPDATA%/Beamer/config.toml`.
/// Missing fields fall back to serde defaults — the file is created on first launch.
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub pause_media: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_language")]
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionConfig {
    /// Ordered list of injection backends to try (e.g. ["ydotool", "clipboard"]).
    #[serde(default = "default_backends")]
    pub backends: Vec<String>,
    #[serde(default)]
    pub debug_logging: bool,
    /// Linux only: which keystroke the clipboard backend sends to paste.
    /// Valid values: "auto" (default — queries the GNOME focus helper
    /// extension to pick per-app; falls back to Ctrl+Shift+V when the
    /// extension is unavailable), "ctrl_v", "ctrl_shift_v".
    /// Overridden at runtime by the `BEAMER_PASTE_SHORTCUT` env var.
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
fn default_true() -> bool { true }


impl Default for Config {
    fn default() -> Self {
        Self {
            recording: RecordingConfig::default(),
            transcription: TranscriptionConfig::default(),
            injection: InjectionConfig::default(),
            appearance: AppearanceConfig::default(),
        }
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            mode: default_mode(),
            pause_media: false,
        }
    }
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            language: default_language(),
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

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let mut config: Config = toml::from_str(&contents)?;
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
        } else {
            let config = Config::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(Self::config_path(), contents)?;
        Ok(())
    }
}

/// Normalize a stored injection backend chain against the current build.
/// Returns `true` if the list changed (caller should re-save).
fn migrate_injection_backends(backends: &mut Vec<String>) -> bool {
        let mut dirty = false;

        // Drop backends that no longer exist in this build. dotool/enigo were
        // removed because XTEST under Ubuntu 26.04+ Xwayland forwards to the
        // Remote Desktop portal and neither worked around it on GNOME Mutter;
        // atspi because its registry deserialization broke against current
        // at-spi2-core. wtype is NOT in this list anymore: it returned in
        // 2026-07 as a first-class backend for wlroots compositors, behind an
        // availability probe that fails fast on GNOME/KDE.
        const REMOVED: &[&str] = &["dotool", "enigo", "atspi"];
        let before = backends.len();
        backends.retain(|b| !REMOVED.contains(&b.as_str()));
        if backends.len() != before {
            dirty = true;
        }

        // Chains saved by builds whose default was ["ydotool", "clipboard"]
        // upgrade to the current default so existing users pick up the gnome
        // and wtype backends. Custom orderings are left alone.
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

/// In-memory cache of API keys read from (or written to) the OS keyring
/// (Windows Credential Manager / Secret Service), keyed by credential name.
///
/// Only *confirmed* state is ever cached: successful reads, and writes/deletes
/// that the keyring itself confirmed. A miss or error never populates the
/// cache — see `load_api_key` and `save_api_key` below. This keeps a
/// transiently locked/unavailable keyring at startup from permanently
/// masking a key that's actually present.
static KEY_CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Read an API key from the OS keyring (keyring crate, service "beamer"),
/// serving from an in-memory cache after the first successful read.
///
/// Negative results (no entry, or a keyring error such as a locked
/// Secret Service collection) are intentionally NOT cached, so a transient
/// keyring failure can't permanently hide a key that's really there —
/// the next call will simply retry the keyring.
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

/// Write or delete an API key in the OS keyring. An empty `value` deletes
/// the credential.
///
/// The keyring is always written first; the in-memory cache is only updated
/// once the keyring operation is confirmed, so the cache can never claim a
/// key that isn't (or is no longer) durably stored. If a write/delete fails
/// (e.g. keyring locked), the cache is left untouched rather than guessed at.
pub fn save_api_key(name: &str, value: &str) {
    if value.is_empty() {
        let result = keyring::Entry::new("beamer", name).and_then(|e| e.delete_credential());
        match result {
            // Deleted, or already absent: either way the keyring holds no
            // value for this name, so the cache shouldn't either.
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

    /// Exercises the cache layer in isolation, without touching the real OS
    /// keyring: pre-populate the static cache directly and confirm
    /// `load_api_key` serves the cached value rather than hitting the
    /// keyring backend at all (which would fail/hang in a headless test
    /// environment).
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

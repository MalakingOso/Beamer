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
    /// Linux only: which keystroke the clipboard backend should send to paste.
    /// "ctrl_shift_v" (default) — works in terminals and pastes as plain text in
    ///   most other apps.
    /// "ctrl_v" — standard paste; some terminals (Warp, Kitty, Alacritty) ignore it.
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

            // Migrate: drop backends that no longer exist in this build.
            // dotool/wtype/enigo were removed because XTEST under Ubuntu 26.04+
            // Xwayland forwards to the Remote Desktop portal and none of them
            // worked around it on GNOME Mutter. atspi was removed because its
            // registry deserialization broke against current at-spi2-core, and
            // ydotool-type covers every app we care about anyway.
            const REMOVED: &[&str] = &["dotool", "wtype", "enigo", "atspi"];
            let before = config.injection.backends.len();
            config.injection.backends.retain(|b| !REMOVED.contains(&b.as_str()));
            if config.injection.backends.len() != before {
                dirty = true;
            }

            // Safety net: never leave the user with an empty backend chain.
            if config.injection.backends.is_empty() {
                config.injection.backends = default_backends();
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

/// Read an API key from Windows Credential Manager (keyring crate, service "beamer").
pub fn load_api_key(name: &str) -> String {
    keyring::Entry::new("beamer", name)
        .and_then(|e| e.get_password())
        .unwrap_or_default()
}

/// Write or delete an API key in Windows Credential Manager.
pub fn save_api_key(name: &str, value: &str) {
    if value.is_empty() {
        if let Ok(entry) = keyring::Entry::new("beamer", name) {
            let _ = entry.delete_credential();
        }
    } else if let Ok(entry) = keyring::Entry::new("beamer", name) {
        let _ = entry.set_password(value);
    }
}

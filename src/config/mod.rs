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
    #[serde(default)]
    pub advanced: AdvancedConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_mode")]
    pub mode: String,
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
    /// "auto" | "uia" | "sendinput" | "clipboard" — selects the text injection strategy
    #[serde(default = "default_preferred_method")]
    pub preferred_method: String,
    #[serde(default)]
    pub debug_logging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    #[serde(default = "default_glow_color")]
    pub glow_color: String,
    #[serde(default = "default_true")]
    pub overlay_enabled: bool,
    #[serde(default)]
    pub auto_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedConfig {
}

fn default_hotkey() -> String { "Ctrl+Space".into() }
fn default_mode() -> String { "hold".into() }
fn default_backend() -> String { "elevenlabs".into() }
fn default_language() -> String { "en".into() }
fn default_preferred_method() -> String { "auto".into() }
fn default_glow_color() -> String { "#4B0082".into() }
fn default_true() -> bool { true }


impl Default for Config {
    fn default() -> Self {
        Self {
            recording: RecordingConfig::default(),
            transcription: TranscriptionConfig::default(),
            injection: InjectionConfig::default(),
            appearance: AppearanceConfig::default(),
            advanced: AdvancedConfig::default(),
        }
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            mode: default_mode(),
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
            preferred_method: default_preferred_method(),
            debug_logging: false,
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            glow_color: default_glow_color(),
            overlay_enabled: default_true(),
            auto_start: false,
        }
    }
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {}
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
            let config: Config = toml::from_str(&contents)?;
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

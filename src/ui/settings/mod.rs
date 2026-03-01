pub mod api_keys_card;
pub mod appearance_card;
pub mod debug_card;
pub mod recording_card;
pub mod transcription_card;
pub mod vocabulary_card;

use dioxus::prelude::*;

use self::api_keys_card::ApiKeysCard;
use self::appearance_card::AppearanceCard;
use self::debug_card::DebugCard;
use self::recording_card::RecordingCard;
use self::transcription_card::TranscriptionCard;
use self::vocabulary_card::VocabularyCard;
use crate::config::Config;
use crate::ui::status_log::StatusLog;

#[derive(Props, Clone, PartialEq)]
pub struct SettingsPageProps {
    pub config: Signal<Config>,
    pub last_injection: Signal<String>,
    pub status_log: Signal<StatusLog>,
}

#[component]
pub fn SettingsPage(props: SettingsPageProps) -> Element {
    let mut config = props.config;
    let last_injection = props.last_injection;
    let mut vocab_terms = use_signal(|| {
        crate::config::vocabulary::Vocabulary::load()
            .map(|v| v.list().to_vec())
            .unwrap_or_default()
    });
    let mut elevenlabs_key = use_signal(|| load_api_key("elevenlabs_api_key"));

    rsx! {
        div { class: "content",
            RecordingCard {
                hotkey: config.read().recording.hotkey.clone(),
                mode: config.read().recording.mode.clone(),
                on_mode_change: move |mode: String| {
                    config.write().recording.mode = mode;
                },
            }

            TranscriptionCard {
                backend: config.read().transcription.backend.clone(),
                language: config.read().transcription.language.clone(),
                on_backend_change: move |backend: String| {
                    config.write().transcription.backend = backend;
                },
                on_language_change: move |lang: String| {
                    config.write().transcription.language = lang;
                },
            }

            ApiKeysCard {
                elevenlabs_key: elevenlabs_key.read().clone(),
                on_elevenlabs_change: move |key: String| {
                    elevenlabs_key.set(key);
                },
            }

            VocabularyCard {
                terms: vocab_terms.read().clone(),
                on_add: move |term: String| {
                    let mut terms = vocab_terms.read().clone();
                    if !terms.contains(&term) {
                        terms.push(term);
                        vocab_terms.set(terms);
                    }
                },
                on_remove: move |term: String| {
                    let mut terms = vocab_terms.read().clone();
                    terms.retain(|t| t != &term);
                    vocab_terms.set(terms);
                },
            }

            AppearanceCard {
                glow_color: config.read().appearance.glow_color.clone(),
                on_color_change: move |color: String| {
                    config.write().appearance.glow_color = color;
                },
            }

            DebugCard {
                last_injection: last_injection.read().clone(),
                debug_logging: config.read().injection.debug_logging,
                on_debug_toggle: move |v: bool| {
                    config.write().injection.debug_logging = v;
                },
                status_log: props.status_log,
            }

            div { class: "footer",
                button {
                    class: "btn btn-primary",
                    onclick: move |_| {
                        let cfg = config.read().clone();
                        if let Err(e) = cfg.save() {
                            tracing::error!("Failed to save config: {}", e);
                        }
                        // Save API keys
                        save_api_key("elevenlabs_api_key", &elevenlabs_key.read());
                        // Save vocabulary
                        if let Ok(mut vocab) = crate::config::vocabulary::Vocabulary::load() {
                            // Clear and re-add all terms
                            let current = vocab.list().to_vec();
                            for term in &current {
                                let _ = vocab.remove(term);
                            }
                            for term in vocab_terms.read().iter() {
                                let _ = vocab.add(term);
                            }
                        }
                        tracing::info!("Settings saved");
                    },
                    "Save Changes"
                }
            }
        }
    }
}

fn load_api_key(name: &str) -> String {
    keyring::Entry::new("beamer", name)
        .and_then(|e| e.get_password())
        .unwrap_or_default()
}

fn save_api_key(name: &str, value: &str) {
    if value.is_empty() {
        if let Ok(entry) = keyring::Entry::new("beamer", name) {
            let _ = entry.delete_credential();
        }
    } else if let Ok(entry) = keyring::Entry::new("beamer", name) {
        let _ = entry.set_password(value);
    }
}

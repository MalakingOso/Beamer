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
    let mut elevenlabs_key = use_signal(|| crate::config::load_api_key("elevenlabs_api_key"));
    let mut mistral_key = use_signal(|| crate::config::load_api_key("mistral_api_key"));

    rsx! {
        div { class: "content",
            RecordingCard {
                hotkey: config.read().recording.hotkey.clone(),
                mode: config.read().recording.mode.clone(),
                pause_media: config.read().recording.pause_media,
                on_hotkey_change: move |hotkey: String| {
                    config.write().recording.hotkey = hotkey;
                },
                on_mode_change: move |mode: String| {
                    config.write().recording.mode = mode;
                },
                on_pause_media_change: move |v: bool| {
                    config.write().recording.pause_media = v;
                },
            }

            TranscriptionCard {
                backend: config.read().transcription.backend.clone(),
                on_backend_change: move |b: String| {
                    config.write().transcription.backend = b;
                },
                language: config.read().transcription.language.clone(),
                on_language_change: move |lang: String| {
                    config.write().transcription.language = lang;
                },
            }

            ApiKeysCard {
                elevenlabs_key: elevenlabs_key.read().clone(),
                on_elevenlabs_change: move |key: String| {
                    elevenlabs_key.set(key);
                },
                mistral_key: mistral_key.read().clone(),
                on_mistral_change: move |key: String| {
                    mistral_key.set(key);
                },
            }

            VocabularyCard {
                terms: vocab_terms.read().clone(),
                on_add: move |term: String| {
                    let mut terms = vocab_terms.read().clone();
                    if !terms.contains(&term) {
                        terms.push(term.clone());
                        vocab_terms.set(terms);
                    }
                    // Persist immediately to disk
                    if let Ok(mut vocab) = crate::config::vocabulary::Vocabulary::load() {
                        if let Err(e) = vocab.add(&term) {
                            tracing::error!("Failed to save vocabulary term: {}", e);
                        }
                    }
                },
                on_remove: move |term: String| {
                    let mut terms = vocab_terms.read().clone();
                    terms.retain(|t| t != &term);
                    vocab_terms.set(terms);
                    // Persist immediately to disk
                    if let Ok(mut vocab) = crate::config::vocabulary::Vocabulary::load() {
                        if let Err(e) = vocab.remove(&term) {
                            tracing::error!("Failed to remove vocabulary term: {}", e);
                        }
                    }
                },
            }

            AppearanceCard {
                pill_enabled: config.read().appearance.pill_enabled,
                on_pill_toggle: move |v: bool| {
                    config.write().appearance.pill_enabled = v;
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
                        crate::config::save_api_key("elevenlabs_api_key", &elevenlabs_key.read());
                        crate::config::save_api_key("mistral_api_key", &mistral_key.read());
                        tracing::info!("Settings saved");
                    },
                    "Save Changes"
                }
            }
        }
    }
}

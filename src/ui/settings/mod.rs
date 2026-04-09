pub mod api_keys_card;
pub mod appearance_card;
pub mod debug_card;
pub mod injection_card;
pub mod recording_card;
pub mod transcription_card;
pub mod update_card;
pub mod vocabulary_card;

use dioxus::prelude::*;

use self::api_keys_card::ApiKeysCard;
use self::appearance_card::AppearanceCard;
use self::debug_card::DebugCard;
use self::injection_card::InjectionCard;
use self::recording_card::RecordingCard;
use self::transcription_card::TranscriptionCard;
use self::update_card::UpdateCard;
use crate::config::Config;
use crate::ui::status_log::StatusLog;
use crate::update::UpdateStatus;

#[derive(Props, Clone, PartialEq)]
pub struct SettingsPageProps {
    pub config: Signal<Config>,
    pub last_injection: Signal<String>,
    pub status_log: Signal<StatusLog>,
    pub update_status: Signal<UpdateStatus>,
}

#[component]
pub fn SettingsPage(props: SettingsPageProps) -> Element {
    let mut config = props.config;
    let last_injection = props.last_injection;
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
                    let _ = config.read().save();
                },
                on_mode_change: move |mode: String| {
                    config.write().recording.mode = mode;
                    let _ = config.read().save();
                },
                on_pause_media_change: move |v: bool| {
                    config.write().recording.pause_media = v;
                    let _ = config.read().save();
                },
            }

            TranscriptionCard {
                backend: config.read().transcription.backend.clone(),
                on_backend_change: move |b: String| {
                    config.write().transcription.backend = b;
                    let _ = config.read().save();
                },
                language: config.read().transcription.language.clone(),
                on_language_change: move |lang: String| {
                    config.write().transcription.language = lang;
                    let _ = config.read().save();
                },
            }

            InjectionCard {
                backends: config.read().injection.backends.clone(),
                on_backends_change: move |backends: Vec<String>| {
                    config.write().injection.backends = backends;
                    let _ = config.read().save();
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

            AppearanceCard {
                pill_enabled: config.read().appearance.pill_enabled,
                on_pill_toggle: move |v: bool| {
                    config.write().appearance.pill_enabled = v;
                    let _ = config.read().save();
                },
                auto_start: config.read().appearance.auto_start,
                on_auto_start_toggle: move |v: bool| {
                    config.write().appearance.auto_start = v;
                    let _ = config.read().save();
                    spawn(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            crate::set_auto_start(v)
                        }).await;
                        match result {
                            Ok(Err(e)) => tracing::error!("Failed to set auto-start: {}", e),
                            Err(e) => tracing::error!("Auto-start task panicked: {}", e),
                            _ => {}
                        }
                    });
                },
            }

            UpdateCard {
                update_status: props.update_status,
                auto_check_updates: config.read().appearance.auto_check_updates,
                on_auto_check_toggle: move |v: bool| {
                    config.write().appearance.auto_check_updates = v;
                    let _ = config.read().save();
                },
            }

            DebugCard {
                last_injection: last_injection.read().clone(),
                debug_logging: config.read().injection.debug_logging,
                on_debug_toggle: move |v: bool| {
                    config.write().injection.debug_logging = v;
                    let _ = config.read().save();
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

//! Settings page: four `SettingsGroup`s (Dictation, Intelligence, Sync &
//! Updates, System), one file per card. Most cards take values and callbacks;
//! this file owns the config handlers, and every change persists at once via
//! `save_config` (API keys go to the OS keyring instead). Signals arrive as
//! props from `App`; see `agent_docs/dioxus_architecture.md`.

pub mod api_keys_card;
pub mod appearance_card;
pub mod debug_card;
pub mod hotkey_picker;
pub mod injection_card;
pub mod layout;
pub mod local_ai_card;
pub mod recording_card;
pub mod sync_card;
pub mod transcription_card;
pub mod update_card;

use dioxus::prelude::*;

use self::api_keys_card::ApiKeysCard;
use self::appearance_card::AppearanceCard;
use self::debug_card::DebugCard;
use self::injection_card::InjectionCard;
use self::layout::SettingsGroup;
use self::local_ai_card::LocalAiCard;
use self::recording_card::RecordingCard;
use self::sync_card::SyncCard;
use self::transcription_card::TranscriptionCard;
use self::update_card::UpdateCard;
use crate::config::Config;
use crate::notes::sync_client::SyncClientHandle;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::ui::status_log::StatusLog;
use crate::update::UpdateStatus;

#[derive(Props, Clone, PartialEq)]
pub struct SettingsPageProps {
    pub config: Signal<Config>,
    pub last_injection: Signal<String>,
    pub status_log: Signal<StatusLog>,
    pub update_status: Signal<UpdateStatus>,
    pub notes: Signal<NoteStore>,
    pub tasks: Signal<TaskStore>,
    pub sync_client: SyncClientHandle,
    /// The component reconcile (`crate::components`): download prompts,
    /// progress and failures for what lives outside the exe.
    pub components: crate::components::Components,
}

/// Apply a mutation to the config signal, then persist it synchronously so a
/// write is never lost on quit.
fn save_config(mut config: Signal<Config>, mutate: impl FnOnce(&mut Config)) {
    mutate(&mut config.write());
    // A failed save must surface: silently keeping the in-memory value while
    // the file still holds the old one reads back as a reverted setting.
    if let Err(e) = config.read().save() {
        tracing::error!("Settings: failed to persist config: {:#}", e);
    }
}

#[component]
pub fn SettingsPage(props: SettingsPageProps) -> Element {
    let config = props.config;
    let last_injection = props.last_injection;
    let mut elevenlabs_key = use_signal(|| crate::config::load_api_key("elevenlabs_api_key"));
    let mut mistral_key = use_signal(|| crate::config::load_api_key("mistral_api_key"));

    rsx! {
        div { class: "content",
            SettingsGroup { title: "Dictation".to_string(),
            RecordingCard {
                hotkey: config.read().recording.hotkey.clone(),
                mode: config.read().recording.mode.clone(),
                pause_media: config.read().recording.pause_media,
                note_hotkey: config.read().recording.note_hotkey.clone(),
                note_mode: config.read().recording.note_mode.clone(),
                done_hotkey: config.read().recording.done_hotkey.clone(),
                done_mode: config.read().recording.done_mode.clone(),
                on_hotkey_change: move |hotkey: String| {
                    save_config(config, |c| c.recording.hotkey = hotkey);
                },
                on_mode_change: move |mode: String| {
                    save_config(config, |c| c.recording.mode = mode);
                },
                on_pause_media_change: move |v: bool| {
                    save_config(config, |c| c.recording.pause_media = v);
                },
                on_note_hotkey_change: move |hotkey: String| {
                    save_config(config, |c| c.recording.note_hotkey = hotkey);
                },
                on_note_mode_change: move |mode: String| {
                    save_config(config, |c| c.recording.note_mode = mode);
                },
                on_done_hotkey_change: move |hotkey: String| {
                    save_config(config, |c| c.recording.done_hotkey = hotkey);
                },
                on_done_mode_change: move |mode: String| {
                    save_config(config, |c| c.recording.done_mode = mode);
                },
            }

            TranscriptionCard {
                backend: config.read().transcription.backend.clone(),
                on_backend_change: move |b: String| {
                    save_config(config, |c| c.transcription.backend = b);
                },
                language: config.read().transcription.language.clone(),
                on_language_change: move |lang: String| {
                    save_config(config, |c| c.transcription.language = lang);
                },
                no_verbatim: config.read().transcription.no_verbatim,
                on_no_verbatim_toggle: move |v: bool| {
                    save_config(config, |c| c.transcription.no_verbatim = v);
                },
            }

            InjectionCard {}
            }

            SettingsGroup { title: "Intelligence".to_string(),
            ApiKeysCard {
                elevenlabs_key: elevenlabs_key.read().clone(),
                on_elevenlabs_change: move |key: String| {
                    // Saved immediately like every other field: nothing hints at
                    // a separate save, so deferring one silently loses keys on close.
                    crate::config::save_api_key("elevenlabs_api_key", &key);
                    elevenlabs_key.set(key);
                },
                mistral_key: mistral_key.read().clone(),
                on_mistral_change: move |key: String| {
                    crate::config::save_api_key("mistral_api_key", &key);
                    mistral_key.set(key);
                },
            }

            LocalAiCard {
                enabled: config.read().llm.enabled,
                base_url: config.read().llm.base_url.clone(),
                request_timeout_ms: config.read().llm.request_timeout_ms,
                connect_timeout_ms: config.read().llm.connect_timeout_ms,
                extract_model: config.read().llm.extract.model.clone(),
                on_enabled_change: move |v: bool| {
                    save_config(config, |c| c.llm.enabled = v);
                },
                on_base_url_change: move |url: String| {
                    save_config(config, |c| c.llm.base_url = url);
                },
                on_connect_timeout_ms_change: move |ms: u64| {
                    save_config(config, |c| c.llm.connect_timeout_ms = ms);
                },
                on_extract_model_change: move |m: String| {
                    save_config(config, |c| c.llm.extract.model = m);
                },
                components: props.components,
            }
            }

            SettingsGroup { title: "Sync & Updates".to_string(),
            SyncCard {
                url: config.read().sync.url.clone(),
                status: props.sync_client.status,
                started_url: props.sync_client.started_url.clone(),
                on_url_change: move |url: String| {
                    save_config(config, |c| c.sync.url = url);
                },
            }

            UpdateCard {
                update_status: props.update_status,
                auto_check_updates: config.read().appearance.auto_check_updates,
                on_auto_check_toggle: move |v: bool| {
                    save_config(config, |c| c.appearance.auto_check_updates = v);
                },
                notes: props.notes,
                tasks: props.tasks,
                components: props.components,
            }
            }

            SettingsGroup { title: "System".to_string(),
            AppearanceCard {
                pill_enabled: config.read().appearance.pill_enabled,
                on_pill_toggle: move |v: bool| {
                    save_config(config, |c| c.appearance.pill_enabled = v);
                },
                auto_start: config.read().appearance.auto_start,
                on_auto_start_toggle: move |v: bool| {
                    save_config(config, |c| c.appearance.auto_start = v);
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

            DebugCard {
                last_injection: last_injection.read().clone(),
                debug_logging: config.read().injection.debug_logging,
                on_debug_toggle: move |v: bool| {
                    save_config(config, |c| c.injection.debug_logging = v);
                    crate::logging::set_debug(v);
                },
                status_log: props.status_log,
            }
            }
        }
    }
}

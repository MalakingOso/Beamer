use dioxus::prelude::*;

use crate::config::Config;
use crate::ui::components::{Card, Select};
use crate::ui::history::TranscriptionHistory;

#[derive(Props, Clone, PartialEq)]
pub struct HomePageProps {
    pub is_recording: Signal<bool>,
    pub history: Signal<TranscriptionHistory>,
    pub config: Signal<Config>,
}

#[component]
pub fn HomePage(props: HomePageProps) -> Element {
    let is_recording = *props.is_recording.read();
    let (status_class, status_label) = if is_recording {
        ("status-dot status-recording", "Recording")
    } else {
        ("status-dot status-ready", "Ready")
    };
    let hotkey = props.config.read().recording.hotkey.clone();

    let recent = props.history.read().recent(5).into_iter().cloned().collect::<Vec<_>>();

    rsx! {
        div { class: "content",
            Card { title: "Status".to_string(),
                div { class: "card-row",
                    div { style: "display: flex; align-items: center; gap: 8px;",
                        div { class: "{status_class}" }
                        span { "{status_label}" }
                    }
                    span { class: "hotkey-display", "{hotkey}" }
                }
            }

            Card { title: "Recent".to_string(),
                if recent.is_empty() {
                    div { class: "empty-state", "No transcriptions yet" }
                } else {
                    for entry in &recent {
                        { render_recent_entry(entry.timestamp.clone(), entry.text.clone()) }
                    }
                }
            }

            Card { title: "Quick Settings".to_string(),
                div { class: "quick-settings-row",
                    span { class: "card-label", "Backend" }
                    Select {
                        value: props.config.read().transcription.backend.clone(),
                        options: vec![
                            ("elevenlabs_realtime".to_string(), "ElevenLabs Realtime".to_string()),
                            ("elevenlabs_batch".to_string(), "ElevenLabs Batch".to_string()),
                        ],
                        onchange: {
                            let mut config = props.config;
                            move |backend: String| {
                                config.write().transcription.backend = backend;
                                let _ = config.read().save();
                            }
                        },
                    }
                }
                div { class: "quick-settings-row",
                    span { class: "card-label", "Language" }
                    Select {
                        value: props.config.read().transcription.language.clone(),
                        options: vec![
                            ("en".to_string(), "English".to_string()),
                            ("es".to_string(), "Spanish".to_string()),
                            ("fr".to_string(), "French".to_string()),
                            ("de".to_string(), "German".to_string()),
                            ("ja".to_string(), "Japanese".to_string()),
                        ],
                        onchange: {
                            let mut config = props.config;
                            move |lang: String| {
                                config.write().transcription.language = lang;
                                let _ = config.read().save();
                            }
                        },
                    }
                }
                div { class: "quick-settings-row",
                    span { class: "card-label", "Mode" }
                    Select {
                        value: props.config.read().recording.mode.clone(),
                        options: vec![
                            ("hold".to_string(), "Hold to Talk".to_string()),
                            ("toggle".to_string(), "Toggle".to_string()),
                        ],
                        onchange: {
                            let mut config = props.config;
                            move |mode: String| {
                                config.write().recording.mode = mode;
                                let _ = config.read().save();
                            }
                        },
                    }
                }
            }
        }
    }
}

fn render_recent_entry(timestamp: String, text: String) -> Element {
    let display_time = chrono::DateTime::parse_from_rfc3339(&timestamp)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| "??:??".to_string());

    let truncated = if text.len() > 80 {
        format!("{}...", &text[..80])
    } else {
        text.clone()
    };

    let text_for_copy = text.clone();
    rsx! {
        div {
            class: "history-entry",
            onclick: move |_| {
                let t = text_for_copy.clone();
                spawn(async move {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(&t);
                    }
                });
            },
            span { class: "entry-time", "{display_time}" }
            span { class: "entry-text", "{truncated}" }
        }
    }
}

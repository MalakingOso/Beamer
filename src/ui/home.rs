use dioxus::prelude::*;

use crate::config::Config;
use crate::orchestrator::RecordingState;
use crate::ui::components::{language_options, truncate_chars, Card, Select};
use crate::ui::history::TranscriptionHistory;
use crate::ui::icons::{IconCheck, IconCopy};

static MODE_OPTIONS: &[(&str, &str)] = &[
    ("hold", "Push to Talk"),
    ("toggle", "Toggle"),
];

#[derive(Props, Clone, PartialEq)]
pub struct HomePageProps {
    pub rec_state: Signal<RecordingState>,
    pub history: Signal<TranscriptionHistory>,
    pub config: Signal<Config>,
}

#[component]
pub fn HomePage(props: HomePageProps) -> Element {
    let (status_class, status_label) = match *props.rec_state.read() {
        RecordingState::Idle => ("status-dot status-ready", "Ready"),
        RecordingState::Recording => ("status-dot status-recording", "Recording"),
        RecordingState::Processing => ("status-dot status-processing", "Processing"),
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
                        RecentEntry {
                            key: "{entry.timestamp}",
                            timestamp: entry.timestamp.clone(),
                            text: entry.text.clone(),
                        }
                    }
                }
            }

            Card { title: "Quick Settings".to_string(),
                div { class: "quick-settings-row",
                    span { class: "card-label", "Language" }
                    Select {
                        value: props.config.read().transcription.language.clone(),
                        options: language_options(),
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
                        options: MODE_OPTIONS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>(),
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

#[derive(Props, Clone, PartialEq)]
struct RecentEntryProps {
    timestamp: String,
    text: String,
}

#[component]
fn RecentEntry(props: RecentEntryProps) -> Element {
    let mut copied = use_signal(|| false);

    let display_time = chrono::DateTime::parse_from_rfc3339(&props.timestamp)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| "??:??".to_string());

    let truncated = truncate_chars(&props.text, 80);

    let text_for_copy = props.text.clone();

    rsx! {
        div {
            class: "history-entry",
            span { class: "entry-time", "{display_time}" }
            span { class: "entry-text", "{truncated}" }
            button {
                class: if *copied.read() { "copy-btn copied" } else { "copy-btn" },
                onclick: move |e| {
                    e.stop_propagation();
                    let t = text_for_copy.clone();
                    spawn(async move {
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            let _ = clipboard.set_text(&t);
                        }
                        copied.set(true);
                        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
                        copied.set(false);
                    });
                },
                if *copied.read() {
                    IconCheck { size: 14, class: "copy-icon check".to_string() }
                } else {
                    IconCopy { size: 14, class: "copy-icon".to_string() }
                }
            }
        }
    }
}

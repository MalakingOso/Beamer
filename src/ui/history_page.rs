use dioxus::prelude::*;

use crate::ui::history::TranscriptionHistory;
use crate::ui::icons::{IconCheck, IconCopy};

#[derive(Props, Clone, PartialEq)]
pub struct HistoryPageProps {
    pub history: Signal<TranscriptionHistory>,
}

#[component]
pub fn HistoryPage(props: HistoryPageProps) -> Element {
    let history = props.history.read();
    let groups = history.grouped_by_day();

    rsx! {
        div { class: "content",
            if groups.is_empty() {
                div { class: "empty-state",
                    div { class: "empty-state-icon", "\u{1F399}" }
                    div { class: "empty-state-text", "No transcriptions yet" }
                    div { class: "empty-state-hint", "Start recording to see your history here" }
                }
            } else {
                for (label, entries) in &groups {
                    div { class: "history-group",
                        div { class: "history-date-header", "{label}" }
                        for entry in entries.iter().rev() {
                            HistoryEntry {
                                key: "{entry.timestamp}",
                                timestamp: entry.timestamp.clone(),
                                text: entry.text.clone(),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct HistoryEntryProps {
    timestamp: String,
    text: String,
}

#[component]
fn HistoryEntry(props: HistoryEntryProps) -> Element {
    let mut copied = use_signal(|| false);

    let display_time = chrono::DateTime::parse_from_rfc3339(&props.timestamp)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| "??:??:??".to_string());

    let text_for_copy = props.text.clone();

    rsx! {
        div {
            class: "history-entry",
            span { class: "entry-time", "{display_time}" }
            span { class: "entry-text", "{props.text}" }
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
                    IconCheck { size: 16, class: "copy-icon check".to_string() }
                } else {
                    IconCopy { size: 16, class: "copy-icon".to_string() }
                }
            }
        }
    }
}

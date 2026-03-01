use dioxus::prelude::*;

use crate::ui::history::TranscriptionHistory;

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
                            { render_history_entry(entry.timestamp.clone(), entry.text.clone()) }
                        }
                    }
                }
            }
        }
    }
}

fn render_history_entry(timestamp: String, text: String) -> Element {
    let display_time = chrono::DateTime::parse_from_rfc3339(&timestamp)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| "??:??:??".to_string());

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
            span { class: "entry-text", "{text}" }
            span { class: "copy-btn", "\u{2398}" }
        }
    }
}

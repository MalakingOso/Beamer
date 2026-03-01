use dioxus::prelude::*;

use crate::ui::components::Card;

#[derive(Props, Clone, PartialEq)]
pub struct RecordingCardProps {
    hotkey: String,
    mode: String,
    on_mode_change: EventHandler<String>,
}

#[component]
pub fn RecordingCard(props: RecordingCardProps) -> Element {
    rsx! {
        Card { title: "Recording".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Hotkey" }
                span { class: "hotkey-display", "{props.hotkey}" }
            }
            div { class: "card-row",
                span { class: "card-label", "Mode" }
                div { class: "radio-group",
                    div {
                        class: "radio-option",
                        onclick: move |_| props.on_mode_change.call("hold".to_string()),
                        div {
                            class: if props.mode == "hold" { "radio-dot selected" } else { "radio-dot" },
                        }
                        span { "Hold" }
                    }
                    div {
                        class: "radio-option",
                        onclick: move |_| props.on_mode_change.call("toggle".to_string()),
                        div {
                            class: if props.mode == "toggle" { "radio-dot selected" } else { "radio-dot" },
                        }
                        span { "Toggle" }
                    }
                }
            }
        }
    }
}

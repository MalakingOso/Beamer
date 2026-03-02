use dioxus::prelude::*;

use crate::ui::components::{Card, Toggle};
use crate::ui::status_log::{LogLevel, StatusLog};

#[derive(Props, Clone, PartialEq)]
pub struct DebugCardProps {
    last_injection: String,
    debug_logging: bool,
    on_debug_toggle: EventHandler<bool>,
    status_log: Signal<StatusLog>,
}

#[component]
pub fn DebugCard(props: DebugCardProps) -> Element {
    let log = props.status_log.read();
    let entries: Vec<_> = log.entries.iter().rev().collect();

    rsx! {
        Card { title: "Debug".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Debug logging" }
                Toggle {
                    value: props.debug_logging,
                    ontoggle: move |v: bool| props.on_debug_toggle.call(v),
                }
            }
            div { class: "card-row",
                span { class: "debug-text", "Last: {props.last_injection}" }
            }
            div { class: "status-log",
                if entries.is_empty() {
                    div { class: "log-empty", "No events yet" }
                } else {
                    for entry in &entries {
                        {
                            let level_class = match entry.level {
                                LogLevel::Info => "log-level-info",
                                LogLevel::Warn => "log-level-warn",
                                LogLevel::Error => "log-level-error",
                            };
                            let level_label = match entry.level {
                                LogLevel::Info => "info",
                                LogLevel::Warn => "warn",
                                LogLevel::Error => "err ",
                            };
                            rsx! {
                                div { class: "log-entry",
                                    span { class: "log-time", "{entry.time}" }
                                    span { class: "log-level {level_class}", "{level_label}" }
                                    span { class: "log-msg", "{entry.message}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

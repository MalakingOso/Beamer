use dioxus::prelude::*;

use crate::ui::components::{Card, Toggle};

#[derive(Props, Clone, PartialEq)]
pub struct AppearanceCardProps {
    pill_enabled: bool,
    on_pill_toggle: EventHandler<bool>,
    auto_start: bool,
    on_auto_start_toggle: EventHandler<bool>,
}

#[component]
pub fn AppearanceCard(props: AppearanceCardProps) -> Element {
    rsx! {
        Card { title: "Appearance".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Recording pill" }
                Toggle {
                    value: props.pill_enabled,
                    ontoggle: move |v: bool| props.on_pill_toggle.call(v),
                }
            }
            div { class: "card-row",
                span { class: "card-label", "Start with Windows" }
                Toggle {
                    value: props.auto_start,
                    ontoggle: move |v: bool| props.on_auto_start_toggle.call(v),
                }
            }
        }
    }
}

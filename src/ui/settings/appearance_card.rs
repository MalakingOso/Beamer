use dioxus::prelude::*;

use crate::ui::components::Card;

#[derive(Props, Clone, PartialEq)]
pub struct AppearanceCardProps {
    glow_color: String,
    on_color_change: EventHandler<String>,
}

#[component]
pub fn AppearanceCard(props: AppearanceCardProps) -> Element {
    rsx! {
        Card { title: "Appearance".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Glow color" }
                div { class: "masked-container",
                    input {
                        class: "input input-mono",
                        value: "{props.glow_color}",
                        placeholder: "#4B0082",
                        oninput: move |e: Event<FormData>| {
                            props.on_color_change.call(e.value().to_string());
                        },
                    }
                    div {
                        class: "color-preview",
                        style: "background-color: {props.glow_color};",
                    }
                }
            }
        }
    }
}

use dioxus::prelude::*;

use crate::ui::components::{Card, MaskedInput};

#[derive(Props, Clone, PartialEq)]
pub struct ApiKeysCardProps {
    elevenlabs_key: String,
    on_elevenlabs_change: EventHandler<String>,
}

#[component]
pub fn ApiKeysCard(props: ApiKeysCardProps) -> Element {
    rsx! {
        Card { title: "API Keys".to_string(),
            div { class: "card-row",
                span { class: "card-label", "ElevenLabs" }
                MaskedInput {
                    value: props.elevenlabs_key.clone(),
                    onchange: move |v: String| props.on_elevenlabs_change.call(v),
                }
            }
        }
    }
}

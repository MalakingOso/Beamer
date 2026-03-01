use dioxus::prelude::*;

use crate::ui::components::{Card, Select};

#[derive(Props, Clone, PartialEq)]
pub struct TranscriptionCardProps {
    backend: String,
    language: String,
    on_backend_change: EventHandler<String>,
    on_language_change: EventHandler<String>,
}

#[component]
pub fn TranscriptionCard(props: TranscriptionCardProps) -> Element {
    let backend_options = vec![
        ("elevenlabs_realtime".to_string(), "ElevenLabs Realtime".to_string()),
        ("elevenlabs_batch".to_string(), "ElevenLabs Batch".to_string()),
    ];

    let language_options = vec![
        ("en".to_string(), "English".to_string()),
        ("es".to_string(), "Spanish".to_string()),
        ("fr".to_string(), "French".to_string()),
        ("de".to_string(), "German".to_string()),
        ("it".to_string(), "Italian".to_string()),
        ("pt".to_string(), "Portuguese".to_string()),
        ("ja".to_string(), "Japanese".to_string()),
        ("ko".to_string(), "Korean".to_string()),
        ("zh".to_string(), "Chinese".to_string()),
    ];

    rsx! {
        Card { title: "Transcription".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Backend" }
                Select {
                    value: props.backend.clone(),
                    options: backend_options,
                    onchange: move |v: String| props.on_backend_change.call(v),
                }
            }
            div { class: "card-row",
                span { class: "card-label", "Language" }
                Select {
                    value: props.language.clone(),
                    options: language_options,
                    onchange: move |v: String| props.on_language_change.call(v),
                }
            }
        }
    }
}

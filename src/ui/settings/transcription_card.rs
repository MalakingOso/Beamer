use dioxus::prelude::*;

use crate::ui::components::{language_options, Card, Select, Toggle};

#[derive(Props, Clone, PartialEq)]
pub struct TranscriptionCardProps {
    backend: String,
    on_backend_change: EventHandler<String>,
    language: String,
    on_language_change: EventHandler<String>,
    no_verbatim: bool,
    on_no_verbatim_toggle: EventHandler<bool>,
}

#[component]
pub fn TranscriptionCard(props: TranscriptionCardProps) -> Element {
    let backend_options = vec![
        ("elevenlabs".to_string(), "ElevenLabs".to_string()),
        ("elevenlabs_batch".to_string(), "ElevenLabs (Batch)".to_string()),
        ("voxtral".to_string(), "Voxtral (Mistral)".to_string()),
        ("voxtral_batch".to_string(), "Voxtral (Batch)".to_string()),
    ];

    let is_voxtral = props.backend == "voxtral" || props.backend == "voxtral_batch";

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
            if !is_voxtral {
                div { class: "card-row",
                    span { class: "card-label", "Language" }
                    Select {
                        value: props.language.clone(),
                        options: language_options(),
                        onchange: move |v: String| props.on_language_change.call(v),
                    }
                }
                // ElevenLabs' `no_verbatim`. Hidden rather than disabled on the
                // Voxtral backends, which have no equivalent — a switch that
                // silently does nothing is worse than a switch that isn't there.
                div { class: "card-row",
                    span { class: "card-label", "Remove filler words" }
                    Toggle {
                        value: props.no_verbatim,
                        ontoggle: move |v: bool| props.on_no_verbatim_toggle.call(v),
                    }
                }
                div { class: "card-row",
                    span { class: "card-label card-label-hint",
                        "Drops \"um\", \"uh\", false starts and stutters instead of transcribing them"
                    }
                }
            }
            if is_voxtral {
                div { class: "card-row",
                    span { class: "card-label card-label-hint", "Language is auto-detected by Voxtral" }
                }
            }
        }
    }
}

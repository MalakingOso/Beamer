use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct OverlayProps {
    text: String,
}

#[component]
pub fn Overlay(props: OverlayProps) -> Element {
    rsx! {
        div { class: "overlay",
            "{props.text}"
        }
    }
}

/// Root component for the standalone overlay window. Receives live transcription
/// text via a shared `Signal<String>` context provided by the main app.
#[component]
pub fn OverlayApp() -> Element {
    let overlay_text: Signal<String> = use_context();
    let text = overlay_text.read().clone();

    rsx! {
        head {
            link { rel: "stylesheet", href: asset!("assets/styles.css") }
        }
        div { class: "overlay",
            "{text}"
        }
    }
}

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

/// Standalone app component for the overlay window. Reads text from context.
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

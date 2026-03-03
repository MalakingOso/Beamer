use dioxus::prelude::*;

/// A small dark pill shown at bottom-center of the screen while recording.
/// Displays a pulsing red dot, animated sound-wave bars, and a "Recording" label.
/// Rendered in its own transparent, click-through, always-on-top window.
#[component]
pub fn RecordingPill() -> Element {
    rsx! {
        div { class: "pill",
            div { class: "pill-dot" }
            div { class: "pill-bars",
                div { class: "bar bar-1" }
                div { class: "bar bar-2" }
                div { class: "bar bar-3" }
                div { class: "bar bar-4" }
                div { class: "bar bar-5" }
            }
            span { class: "pill-label", "Recording" }
        }
    }
}

use dioxus::prelude::*;

/// A small dark glass pill shown at bottom-center of the screen while
/// recording — VibeTyper-style: purple-gradient waveform bars plus an elapsed
/// timer while recording, "Transcribing…" while processing. Rendered in its
/// own transparent, click-through, always-on-top window; state transitions
/// are driven from app.rs via the `beamerSetState` head-script.
#[component]
pub fn RecordingPill() -> Element {
    rsx! {
        div { class: "pill",
            div { class: "pill-bars",
                div { class: "bar bar-1" }
                div { class: "bar bar-2" }
                div { class: "bar bar-3" }
                div { class: "bar bar-4" }
                div { class: "bar bar-5" }
            }
            span { class: "pill-timer", "0:00" }
            span { class: "pill-label", "Transcribing…" }
        }
    }
}

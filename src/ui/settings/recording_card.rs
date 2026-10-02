//! Settings → Dictation → Recording: the dictation hotkey and mode, pause-media,
//! and the note-capture hotkey and mode (the second hotkey that sends speech to
//! a sticky note). Hotkey strings are read by `src/hotkey/`.

use dioxus::prelude::*;

use crate::ui::components::Toggle;
use crate::ui::settings::hotkey_picker::{CaptureModeRadio, HotkeyPicker};
use crate::ui::settings::layout::SubSection;

/// Chord proposed when note capture is switched on. The serde default stays
/// empty so a fresh config file steals no chord. Not Ctrl+Super+Space:
/// dictation's `Ctrl+Super` is its prefix and would fire first
/// (see `hotkey::matching_binding`).
const DEFAULT_NOTE_HOTKEY: &str = "Ctrl+Alt+Space";

/// Chord proposed when Done capture is switched on. A different trigger key
/// from both other chords, so only the modifiers could ever confuse them.
const DEFAULT_DONE_HOTKEY: &str = "Ctrl+Alt+D";

#[derive(Props, Clone, PartialEq)]
pub struct RecordingCardProps {
    hotkey: String,
    mode: String,
    pause_media: bool,
    /// Empty means note capture is off; no binding is registered.
    note_hotkey: String,
    note_mode: String,
    on_hotkey_change: EventHandler<String>,
    on_mode_change: EventHandler<String>,
    on_pause_media_change: EventHandler<bool>,
    on_note_hotkey_change: EventHandler<String>,
    on_note_mode_change: EventHandler<String>,
    /// Empty means Done capture is off.
    done_hotkey: String,
    done_mode: String,
    on_done_hotkey_change: EventHandler<String>,
    on_done_mode_change: EventHandler<String>,
}

#[component]
pub fn RecordingCard(props: RecordingCardProps) -> Element {
    let note_enabled = !props.note_hotkey.trim().is_empty();
    let done_enabled = !props.done_hotkey.trim().is_empty();

    rsx! {
        SubSection { label: "Recording".to_string(),
            div { class: "card-row card-row-top",
                span { class: "card-label", "Hotkey" }
                HotkeyPicker {
                    hotkey: props.hotkey.clone(),
                    on_change: move |h: String| props.on_hotkey_change.call(h),
                }
            }
            div { class: "card-row",
                span { class: "card-label", "Mode" }
                CaptureModeRadio {
                    mode: props.mode.clone(),
                    on_change: move |m: String| props.on_mode_change.call(m),
                }
            }
            div { class: "card-row",
                span { class: "card-label", "Pause media" }
                Toggle {
                    value: props.pause_media,
                    ontoggle: move |v| props.on_pause_media_change.call(v),
                }
            }

            div { class: "card-row",
                span { class: "card-label", "Note capture" }
                Toggle {
                    value: note_enabled,
                    // Off clears the chord: empty is the only "register
                    // nothing" state the hotkey layer reads.
                    ontoggle: move |on: bool| {
                        let next = if on { DEFAULT_NOTE_HOTKEY } else { "" };
                        props.on_note_hotkey_change.call(next.to_string());
                    },
                }
            }
            if note_enabled {
                div { class: "card-row",
                    span { class: "card-label", "Note hotkey" }
                    HotkeyPicker {
                        hotkey: props.note_hotkey.clone(),
                        on_change: move |h: String| props.on_note_hotkey_change.call(h),
                    }
                }
                div { class: "card-row",
                    span { class: "card-label", "Note mode" }
                    CaptureModeRadio {
                        mode: props.note_mode.clone(),
                        on_change: move |m: String| props.on_note_mode_change.call(m),
                    }
                }
            }

            div { class: "card-row",
                span { class: "card-label", "Done capture" }
                Toggle {
                    value: done_enabled,
                    ontoggle: move |on: bool| {
                        let next = if on { DEFAULT_DONE_HOTKEY } else { "" };
                        props.on_done_hotkey_change.call(next.to_string());
                    },
                }
            }
            if done_enabled {
                div { class: "card-row",
                    span { class: "card-label", "Done hotkey" }
                    HotkeyPicker {
                        hotkey: props.done_hotkey.clone(),
                        on_change: move |h: String| props.on_done_hotkey_change.call(h),
                    }
                }
                div { class: "card-row",
                    span { class: "card-label", "Done mode" }
                    CaptureModeRadio {
                        mode: props.done_mode.clone(),
                        on_change: move |m: String| props.on_done_mode_change.call(m),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::HotkeyConfig;

    /// Dictation's chord must not be a prefix of the note chord, or dictation
    /// would fire first on the way to the note trigger.
    #[test]
    fn the_proposed_note_chord_does_not_pass_through_the_dictation_chord() {
        let dictation = HotkeyConfig::parse("Ctrl+Super", false).expect("parses");
        let note = HotkeyConfig::parse(DEFAULT_NOTE_HOTKEY, true).expect("parses");

        assert_ne!(
            dictation.trigger_vk, note.trigger_vk,
            "chords sharing a trigger key can only be told apart by modifiers"
        );
    }

    #[test]
    fn the_three_proposed_chords_are_distinct_bindings() {
        let note = HotkeyConfig::parse(DEFAULT_NOTE_HOTKEY, true).expect("parses");
        let done = HotkeyConfig::parse(DEFAULT_DONE_HOTKEY, false).expect("parses");
        assert_ne!(note.trigger_vk, done.trigger_vk);
    }
}

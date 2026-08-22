use dioxus::prelude::*;

use crate::ui::components::{Card, Toggle};
use crate::ui::settings::hotkey_picker::{CaptureModeRadio, HotkeyPicker};

/// Chord proposed when note capture is switched on from the UI.
///
/// Deliberately NOT the serde default for `note_hotkey`, which stays empty so
/// no chord is ever stolen from another app by a config file appearing. This
/// is only a starting point for a user who has explicitly asked for the
/// feature, and it is theirs to change from the picker right below the switch.
///
/// Ctrl+Alt+Space, not Ctrl+Super+Space: dictation's `Ctrl+Super` is a strict
/// prefix of the latter, so pressing Ctrl then Super would start a dictation
/// recording before Space was ever reached. See `linux_hotkey::matching_binding`.
const DEFAULT_NOTE_HOTKEY: &str = "Ctrl+Alt+Space";

#[derive(Props, Clone, PartialEq)]
pub struct RecordingCardProps {
    hotkey: String,
    mode: String,
    pause_media: bool,
    /// Empty means note capture is off entirely — no binding is registered.
    note_hotkey: String,
    note_mode: String,
    on_hotkey_change: EventHandler<String>,
    on_mode_change: EventHandler<String>,
    on_pause_media_change: EventHandler<bool>,
    on_note_hotkey_change: EventHandler<String>,
    on_note_mode_change: EventHandler<String>,
}

#[component]
pub fn RecordingCard(props: RecordingCardProps) -> Element {
    let note_enabled = !props.note_hotkey.trim().is_empty();

    rsx! {
        Card { title: "Recording".to_string(),
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
                    // Off clears the chord rather than remembering it: an empty
                    // string is the one state the hotkey layer reads as
                    // "register nothing", so off must genuinely mean unbound.
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::HotkeyConfig;

    /// The chord the switch proposes must be one the engine can actually
    /// register — an unparseable suggestion would turn the feature on while
    /// binding nothing, which looks identical to a broken hotkey.
    #[test]
    fn the_proposed_note_chord_parses() {
        let parsed = HotkeyConfig::parse(DEFAULT_NOTE_HOTKEY, true).expect("must parse");
        assert!(parsed.ctrl && parsed.alt && !parsed.shift);
        assert_eq!(parsed.trigger_vk, 0x20, "Space");
    }

    /// Dictation's default chord must not be a prefix of the note chord.
    ///
    /// `matching_binding` compares the modifier set held *at the moment the
    /// trigger goes down*. If the note chord's trigger were reached only after
    /// passing through the dictation chord's exact state, dictation would fire
    /// first, every time. This is why the default is not Ctrl+Super+Space.
    #[test]
    fn the_proposed_note_chord_does_not_pass_through_the_dictation_chord() {
        let dictation = HotkeyConfig::parse("Ctrl+Super", false).expect("parses");
        let note = HotkeyConfig::parse(DEFAULT_NOTE_HOTKEY, true).expect("parses");

        // Distinct triggers is the strong form: no press order can reach the
        // note trigger via the dictation trigger.
        assert_ne!(
            dictation.trigger_vk, note.trigger_vk,
            "chords sharing a trigger key can only be told apart by modifiers"
        );
    }
}

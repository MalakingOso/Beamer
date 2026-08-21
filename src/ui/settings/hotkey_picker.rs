use dioxus::prelude::*;

use crate::ui::components::Select;

/// Split "Ctrl+Space" into (ctrl, alt, shift, win, key).
pub fn parse_hotkey_parts(hotkey: &str) -> (bool, bool, bool, bool, String) {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut win = false;
    let mut key = String::new();

    for part in hotkey.split('+') {
        match part.trim().to_uppercase().as_str() {
            "CTRL" | "CONTROL" => ctrl = true,
            "ALT" | "OPTION" => alt = true,
            "SHIFT" => shift = true,
            "SUPER" | "WIN" | "CMD" | "COMMAND" | "META" => win = true,
            _ => key = normalize_key(part.trim()),
        }
    }

    // When Win is active, it IS the trigger — no separate key needed.
    // Otherwise default to Space.
    if key.is_empty() && !win {
        key = "Space".to_string();
    }

    (ctrl, alt, shift, win, key)
}

/// Reassemble modifier+key into a hotkey string.
/// Uses "Super" for Win key. Omits key when Win is the trigger.
pub fn format_hotkey(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> String {
    let mut parts = Vec::new();
    if ctrl { parts.push("Ctrl"); }
    if alt { parts.push("Alt"); }
    if shift { parts.push("Shift"); }
    if win { parts.push("Super"); }
    if !key.is_empty() {
        parts.push(key);
    }
    parts.join("+")
}

/// Normalize long-form key names to what global_hotkey expects.
pub fn normalize_key(key: &str) -> String {
    let upper = key.to_uppercase();
    match upper.as_str() {
        "ARROWUP" => "Up".into(),
        "ARROWDOWN" => "Down".into(),
        "ARROWLEFT" => "Left".into(),
        "ARROWRIGHT" => "Right".into(),
        "PAGEUP" => "PageUp".into(),
        "PAGEDOWN" => "PageDown".into(),
        s if s.starts_with("KEY") && s.len() == 4 => s[3..].to_string(),
        s if s.starts_with("DIGIT") && s.len() == 6 => s[5..].to_string(),
        _ => {
            // Title-case: first char upper, rest lower
            let mut chars = key.chars();
            match chars.next() {
                Some(c) => {
                    let first: String = c.to_uppercase().collect();
                    let rest: String = chars.collect::<String>().to_lowercase();
                    format!("{first}{rest}")
                }
                None => key.to_string(),
            }
        }
    }
}

/// Dropdown options for the key selector.
/// Fixed content — built once as a static slice rather than reconstructed on every render.
static KEY_OPTIONS: &[(&str, &str)] = &[
    // Common keys
    ("Space", "Space"),
    ("Enter", "Enter"),
    ("Tab", "Tab"),
    ("Backspace", "Backspace"),
    ("Delete", "Delete"),
    ("Insert", "Insert"),
    ("Home", "Home"),
    ("End", "End"),
    ("PageUp", "PageUp"),
    ("PageDown", "PageDown"),
    // Letters A-Z
    ("A", "A"), ("B", "B"), ("C", "C"), ("D", "D"), ("E", "E"),
    ("F", "F"), ("G", "G"), ("H", "H"), ("I", "I"), ("J", "J"),
    ("K", "K"), ("L", "L"), ("M", "M"), ("N", "N"), ("O", "O"),
    ("P", "P"), ("Q", "Q"), ("R", "R"), ("S", "S"), ("T", "T"),
    ("U", "U"), ("V", "V"), ("W", "W"), ("X", "X"), ("Y", "Y"),
    ("Z", "Z"),
    // Digits 0-9
    ("0", "0"), ("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"),
    ("5", "5"), ("6", "6"), ("7", "7"), ("8", "8"), ("9", "9"),
    // F-keys
    ("F1", "F1"), ("F2", "F2"), ("F3", "F3"), ("F4", "F4"),
    ("F5", "F5"), ("F6", "F6"), ("F7", "F7"), ("F8", "F8"),
    ("F9", "F9"), ("F10", "F10"), ("F11", "F11"), ("F12", "F12"),
    // Arrows
    ("Up", "Up"),
    ("Down", "Down"),
    ("Left", "Left"),
    ("Right", "Right"),
    // Punctuation / symbols
    ("-", "Minus (-)"),
    ("=", "Equal (=)"),
    ("[", "Left Bracket ([)"),
    ("]", "Right Bracket (])"),
    ("\\", "Backslash (\\)"),
    (";", "Semicolon (;)"),
    ("'", "Quote (')"),
    (",", "Comma (,)"),
    (".", "Period (.)"),
    ("/", "Slash (/)"),
    ("`", "Backtick (`)"),
];

#[derive(Props, Clone, PartialEq)]
pub struct HotkeyPickerProps {
    /// Current chord, e.g. "Ctrl+Super" or "Ctrl+Alt+Space".
    pub hotkey: String,
    /// Emits the reassembled chord. The caller persists it.
    pub on_change: EventHandler<String>,
}

/// Modifier pills plus a trigger-key dropdown, shared by the dictation and
/// note-capture rows so the two chords can never be parsed differently.
#[component]
pub fn HotkeyPicker(props: HotkeyPickerProps) -> Element {
    let (ctrl, alt, shift, win, key) = parse_hotkey_parts(&props.hotkey);
    // Win-as-trigger is the invariant for every closure below, not just the
    // Win pill's. Without this, a hand-edited "Ctrl+Super+Space" seeds
    // key = "Space", and clicking Ctrl would emit "Super+Space" — the exact
    // Super+<key> chord the engine silently degrades (see todo.md). Clearing
    // it here normalizes such a chord on first interaction instead.
    let key = if win { String::new() } else { key };

    rsx! {
        div { class: "hotkey-picker",
            div { class: "hotkey-mods",
                ModPill { label: "Ctrl", active: ctrl, on_click: {
                    let key = key.clone();
                    move |_| {
                        props.on_change.call(format_hotkey(!ctrl, alt, shift, win, &key));
                    }
                }}
                ModPill { label: "Alt", active: alt, on_click: {
                    let key = key.clone();
                    move |_| {
                        props.on_change.call(format_hotkey(ctrl, !alt, shift, win, &key));
                    }
                }}
                ModPill { label: "Shift", active: shift, on_click: {
                    let key = key.clone();
                    move |_| {
                        props.on_change.call(format_hotkey(ctrl, alt, !shift, win, &key));
                    }
                }}
                ModPill { label: "Win", active: win, on_click: {
                    let key = key.clone();
                    move |_| {
                        let new_win = !win;
                        // When Win is toggled on, it becomes the trigger (drop key).
                        // When toggled off, restore Space as default trigger.
                        let effective_key = if new_win { "" } else if key.is_empty() { "Space" } else { &key };
                        props.on_change.call(format_hotkey(ctrl, alt, shift, new_win, effective_key));
                    }
                }}
            }
            if !win {
                Select {
                    value: key.clone(),
                    options: KEY_OPTIONS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>(),
                    onchange: move |new_key: String| {
                        props.on_change.call(format_hotkey(ctrl, alt, shift, win, &new_key));
                    },
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct CaptureModeRadioProps {
    /// "hold" or "toggle".
    pub mode: String,
    pub on_change: EventHandler<String>,
}

/// Push-to-talk vs toggle. Shared so both capture modes read identically.
#[component]
pub fn CaptureModeRadio(props: CaptureModeRadioProps) -> Element {
    rsx! {
        div { class: "radio-group",
            div {
                class: "radio-option",
                onclick: move |_| props.on_change.call("hold".to_string()),
                div {
                    class: if props.mode == "hold" { "radio-dot selected" } else { "radio-dot" },
                }
                span { "Push to Talk" }
            }
            div {
                class: "radio-option",
                onclick: move |_| props.on_change.call("toggle".to_string()),
                div {
                    class: if props.mode == "toggle" { "radio-dot selected" } else { "radio-dot" },
                }
                span { "Toggle" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ModPillProps {
    label: &'static str,
    active: bool,
    on_click: EventHandler<()>,
}

#[component]
fn ModPill(props: ModPillProps) -> Element {
    let class = if props.active { "mod-pill active" } else { "mod-pill" };
    rsx! {
        button {
            class: "{class}",
            onclick: move |_| props.on_click.call(()),
            "{props.label}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::{HotkeyConfig, VK_LWIN};

    #[test]
    fn win_only_chord_keeps_super_as_the_trigger() {
        let (ctrl, alt, shift, win, key) = parse_hotkey_parts("Ctrl+Super");
        assert!(ctrl && win && !alt && !shift);
        assert_eq!(key, "", "Super IS the trigger — no separate key may be invented");
    }

    #[test]
    fn a_keyed_chord_round_trips_through_the_picker() {
        let (ctrl, alt, shift, win, key) = parse_hotkey_parts("Ctrl+Alt+Space");
        assert_eq!(format_hotkey(ctrl, alt, shift, win, &key), "Ctrl+Alt+Space");
    }

    /// The picker and `HotkeyConfig::parse` are two independent readers of the
    /// same string format. Nothing in the type system keeps them in step, so
    /// what the UI writes must be pinned against what the engine registers —
    /// a picker that emits a chord the engine drops fails silently.
    #[test]
    fn what_the_picker_emits_is_what_the_engine_registers() {
        let cases: &[(&str, bool, bool, bool, u32)] = &[
            // chord,            ctrl,  alt,   shift, trigger_vk
            ("Ctrl+Alt+Space",   true,  true,  false, 0x20),
            ("Ctrl+Super",       true,  false, false, VK_LWIN),
            ("Ctrl+Shift+N",     true,  false, true,  0x4E),
            ("Alt+F9",           false, true,  false, 0x78),
        ];

        for &(chord, ctrl, alt, shift, vk) in cases {
            let (c, a, s, w, k) = parse_hotkey_parts(chord);
            let emitted = format_hotkey(c, a, s, w, &k);
            let parsed = HotkeyConfig::parse(&emitted, false)
                .unwrap_or_else(|| panic!("engine rejected picker output {emitted:?}"));

            assert_eq!(parsed.ctrl, ctrl, "ctrl mismatch for {chord}");
            assert_eq!(parsed.alt, alt, "alt mismatch for {chord}");
            assert_eq!(parsed.shift, shift, "shift mismatch for {chord}");
            assert_eq!(parsed.trigger_vk, vk, "trigger mismatch for {chord}");
        }
    }

    /// Documents a KNOWN FOOTGUN, it does not endorse it. Super is expressible
    /// only as a *trigger*: `HotkeyConfig` has no Meta modifier field, so
    /// "Ctrl+Super+Space" silently registers as plain Ctrl+Space. todo.md
    /// tracks the fix (either a `win` field threaded through `Modifiers`, or
    /// `parse()` returning `None` for `Super+<key>` so it fails safe).
    ///
    /// The picker cannot currently emit such a chord — its Win pill drops the
    /// trigger key — so this guards the hand-edited-config path. When the fix
    /// lands, this test SHOULD fail; update it and teach the picker to render
    /// Super as a modifier alongside a key.
    #[test]
    fn super_plus_a_key_is_silently_degraded_by_the_engine() {
        let parsed = HotkeyConfig::parse("Ctrl+Super+Space", false).expect("parses");
        assert_eq!(parsed.trigger_vk, 0x20, "Space wins the trigger slot");
        assert!(parsed.ctrl);

        // The picker never produces one, and normalizes one it is handed:
        // `HotkeyPicker` clears the key whenever Win is on, so every modifier
        // pill reassembles the chord as Super-as-trigger.
        let (c, a, sh, w, k) = parse_hotkey_parts("Ctrl+Super+Space");
        assert!(w, "Win reads as on");
        assert_eq!(k, "Space", "the raw parse still carries the stray key");
        let normalized = if w { String::new() } else { k };
        assert_eq!(format_hotkey(c, a, sh, w, &normalized), "Ctrl+Super");
        // ...and toggling Ctrl off from that state stays safe, rather than
        // emitting "Super+Space".
        assert_eq!(format_hotkey(!c, a, sh, w, &normalized), "Super");
    }
}

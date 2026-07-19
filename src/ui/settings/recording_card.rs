use dioxus::prelude::*;
use crate::ui::components::{Card, Select, Toggle};

#[derive(Props, Clone, PartialEq)]
pub struct RecordingCardProps {
    hotkey: String,
    mode: String,
    pause_media: bool,
    on_hotkey_change: EventHandler<String>,
    on_mode_change: EventHandler<String>,
    on_pause_media_change: EventHandler<bool>,
}

/// Split "Ctrl+Space" into (ctrl, alt, shift, win, key).
fn parse_hotkey_parts(hotkey: &str) -> (bool, bool, bool, bool, String) {
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
fn format_hotkey(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> String {
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
fn normalize_key(key: &str) -> String {
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

#[component]
pub fn RecordingCard(props: RecordingCardProps) -> Element {
    let (ctrl, alt, shift, win, key) = parse_hotkey_parts(&props.hotkey);

    rsx! {
        Card { title: "Recording".to_string(),
            div { class: "card-row card-row-top",
                span { class: "card-label", "Hotkey" }
                div { class: "hotkey-picker",
                    div { class: "hotkey-mods",
                        ModPill { label: "Ctrl", active: ctrl, on_click: {
                            let key = key.clone();
                            move |_| {
                                props.on_hotkey_change.call(format_hotkey(!ctrl, alt, shift, win, &key));
                            }
                        }}
                        ModPill { label: "Alt", active: alt, on_click: {
                            let key = key.clone();
                            move |_| {
                                props.on_hotkey_change.call(format_hotkey(ctrl, !alt, shift, win, &key));
                            }
                        }}
                        ModPill { label: "Shift", active: shift, on_click: {
                            let key = key.clone();
                            move |_| {
                                props.on_hotkey_change.call(format_hotkey(ctrl, alt, !shift, win, &key));
                            }
                        }}
                        ModPill { label: "Win", active: win, on_click: {
                            let key = key.clone();
                            move |_| {
                                let new_win = !win;
                                // When Win is toggled on, it becomes the trigger (drop key).
                                // When toggled off, restore Space as default trigger.
                                let effective_key = if new_win { "" } else if key.is_empty() { "Space" } else { &key };
                                props.on_hotkey_change.call(format_hotkey(ctrl, alt, shift, new_win, effective_key));
                            }
                        }}
                    }
                    if !win {
                        Select {
                            value: key.clone(),
                            options: KEY_OPTIONS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>(),
                            onchange: move |new_key: String| {
                                props.on_hotkey_change.call(format_hotkey(ctrl, alt, shift, win, &new_key));
                            },
                        }
                    }
                }
            }
            div { class: "card-row",
                span { class: "card-label", "Mode" }
                div { class: "radio-group",
                    div {
                        class: "radio-option",
                        onclick: move |_| props.on_mode_change.call("hold".to_string()),
                        div {
                            class: if props.mode == "hold" { "radio-dot selected" } else { "radio-dot" },
                        }
                        span { "Push to Talk" }
                    }
                    div {
                        class: "radio-option",
                        onclick: move |_| props.on_mode_change.call("toggle".to_string()),
                        div {
                            class: if props.mode == "toggle" { "radio-dot selected" } else { "radio-dot" },
                        }
                        span { "Toggle" }
                    }
                }
            }
            div { class: "card-row",
                span { class: "card-label", "Pause media" }
                Toggle {
                    value: props.pause_media,
                    ontoggle: move |v| props.on_pause_media_change.call(v),
                }
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

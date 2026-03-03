use dioxus::prelude::*;
use crate::ui::components::{Card, Toggle};

#[derive(Props, Clone, PartialEq)]
pub struct RecordingCardProps {
    hotkey: String,
    mode: String,
    pause_media: bool,
    on_hotkey_change: EventHandler<String>,
    on_mode_change: EventHandler<String>,
    on_pause_media_change: EventHandler<bool>,
}

#[component]
pub fn RecordingCard(props: RecordingCardProps) -> Element {
    let mut recording = use_signal(|| false);

    rsx! {
        Card { title: "Recording".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Hotkey" }
                if *recording.read() {
                    // Capture zone — full-width, auto-focused, catches keydown
                    div {
                        class: "hotkey-capture-zone",
                        tabindex: 0,
                        onmounted: move |e| async move {
                            let _ = e.set_focus(true).await;
                        },
                        onfocusout: move |_| {
                            recording.set(false);
                        },
                        onkeydown: move |e: Event<KeyboardData>| {
                            e.prevent_default();

                            let key = e.key();

                            if key == Key::Escape {
                                recording.set(false);
                                return;
                            }

                            // Ignore modifier-only presses
                            if matches!(key, Key::Control | Key::Shift | Key::Alt | Key::Meta) {
                                return;
                            }

                            let mods = e.modifiers();
                            let mut parts: Vec<&str> = Vec::new();
                            if mods.contains(Modifiers::CONTROL) { parts.push("Ctrl"); }
                            if mods.contains(Modifiers::ALT) { parts.push("Alt"); }
                            if mods.contains(Modifiers::SHIFT) { parts.push("Shift"); }
                            if mods.contains(Modifiers::META) { parts.push("Win"); }

                            let key_name = format_key_name(&key);
                            let mut combo_parts: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
                            combo_parts.push(key_name);
                            let combo = combo_parts.join("+");

                            recording.set(false);
                            props.on_hotkey_change.call(combo);
                        },
                        "Press shortcut..."
                    }
                } else {
                    // Idle state — keycap display + record button
                    div { class: "hotkey-row",
                        if props.hotkey.is_empty() {
                            span { class: "hotkey-placeholder", "Not set" }
                        } else {
                            {props.hotkey.split('+').enumerate().map(|(i, part)| {
                                rsx! {
                                    if i > 0 {
                                        span { class: "keycap-separator", "+" }
                                    }
                                    span { class: "keycap", "{part.trim()}" }
                                }
                            })}
                        }
                        button {
                            class: "hotkey-record-btn",
                            onclick: move |_| {
                                recording.set(true);
                            },
                            "Record"
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

fn format_key_name(key: &Key) -> String {
    match key {
        Key::Character(c) if c == " " => "Space".into(),
        Key::Character(c) => c.to_uppercase(),
        Key::Backspace => "Backspace".into(),
        Key::Tab => "Tab".into(),
        Key::Enter => "Enter".into(),
        Key::Escape => "Escape".into(),
        Key::Delete => "Delete".into(),
        Key::ArrowUp => "Up".into(),
        Key::ArrowDown => "Down".into(),
        Key::ArrowLeft => "Left".into(),
        Key::ArrowRight => "Right".into(),
        Key::F1 => "F1".into(), Key::F2 => "F2".into(), Key::F3 => "F3".into(),
        Key::F4 => "F4".into(), Key::F5 => "F5".into(), Key::F6 => "F6".into(),
        Key::F7 => "F7".into(), Key::F8 => "F8".into(), Key::F9 => "F9".into(),
        Key::F10 => "F10".into(), Key::F11 => "F11".into(), Key::F12 => "F12".into(),
        Key::End => "End".into(),
        Key::Home => "Home".into(),
        Key::Insert => "Insert".into(),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        _ => format!("{:?}", key),
    }
}

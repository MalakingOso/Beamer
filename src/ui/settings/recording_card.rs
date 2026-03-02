use dioxus::prelude::*;
use crate::ui::components::Card;

#[derive(Props, Clone, PartialEq)]
pub struct RecordingCardProps {
    hotkey: String,
    mode: String,
    on_hotkey_change: EventHandler<String>,
    on_mode_change: EventHandler<String>,
}

#[component]
pub fn RecordingCard(props: RecordingCardProps) -> Element {
    let mut listening = use_signal(|| false);

    rsx! {
        Card { title: "Recording".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Hotkey" }
                button {
                    class: if *listening.read() { "hotkey-btn listening" } else { "hotkey-btn" },
                    tabindex: 0,
                    onclick: move |_| {
                        listening.set(true);
                    },
                    onkeydown: move |e: Event<KeyboardData>| {
                        if !*listening.read() { return; }
                        e.prevent_default();

                        let key = e.key();

                        if key == Key::Escape {
                            listening.set(false);
                            return;
                        }

                        // Wait for a non-modifier key to complete the combo
                        if matches!(key, Key::Control | Key::Shift | Key::Alt | Key::Meta) {
                            return;
                        }

                        let modifiers = e.modifiers();
                        let mut parts = Vec::new();
                        if modifiers.contains(Modifiers::CONTROL) { parts.push("Ctrl"); }
                        if modifiers.contains(Modifiers::ALT) { parts.push("Alt"); }
                        if modifiers.contains(Modifiers::SHIFT) { parts.push("Shift"); }
                        if modifiers.contains(Modifiers::META) { parts.push("Win"); }

                        let key_name = format_key_name(&key);
                        parts.push(&key_name);

                        let combo = parts.join("+");
                        listening.set(false);
                        props.on_hotkey_change.call(combo);
                    },
                    if *listening.read() {
                        "Press a key combo..."
                    } else {
                        "{props.hotkey}"
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
                        span { "Hold" }
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

# Taskbar Icon + Hotkey Picker Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Set the Beamer icon on the Windows taskbar and add a press-to-record hotkey picker in settings.

**Architecture:** Feature 1 is a one-liner on WindowBuilder. Feature 2 adds a "listening" mode to RecordingCard that captures keyboard events, stores the combo as a string, and on save uses DesktopService's `create_shortcut`/`remove_shortcut` to swap the hotkey at runtime.

**Tech Stack:** Dioxus 0.7.3, tao (via dioxus::desktop::tao), global-hotkey (via dioxus::desktop)

---

### Task 1: Set Window Icon on Taskbar

**Files:**
- Modify: `src/ui/mod.rs:15-32`

**Step 1: Add window icon to WindowBuilder**

In `src/ui/mod.rs`, load the icon PNG and pass it to `.with_window_icon()`:

```rust
use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::desktop::tao::window::Icon;
use dioxus::prelude::*;

pub fn launch_app() {
    // Load window icon (same PNG the tray uses)
    let icon_bytes = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(icon_bytes).expect("Failed to load window icon");
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let window_icon = Icon::from_rgba(rgba.into_raw(), w, h).expect("Failed to create window icon");

    LaunchBuilder::new()
        .with_cfg(
            Config::new()
                .with_window(
                    WindowBuilder::new()
                        .with_title("Beamer")
                        .with_window_icon(Some(window_icon))
                        .with_visible(false)
                        .with_decorations(false)
                        .with_transparent(true)
                        .with_inner_size(dioxus::desktop::LogicalSize::new(500.0_f64, 600.0_f64)),
                )
                .with_background_color((0, 0, 0, 0))
                .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                .with_exits_when_last_window_closes(false),
        )
        .launch(app::App);
}
```

**Step 2: Build and verify**

Run: `cargo build`
Expected: Compiles. When run, the taskbar shows the Beamer icon.

**Step 3: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat: set Beamer icon on Windows taskbar"
```

---

### Task 2: Add Press-to-Record Hotkey Picker UI

**Files:**
- Modify: `src/ui/settings/recording_card.rs`
- Modify: `assets/styles.css`

**Step 1: Update RecordingCard to add hotkey change handler and listening state**

Replace `recording_card.rs` with a component that has:
- A `listening` signal (bool) — when true, captures key events
- A button showing the current hotkey. Clicking enters listening mode.
- An `onkeydown` handler on the button that captures modifier+key combos
- Escape cancels listening. Any valid combo updates the hotkey.
- New prop: `on_hotkey_change: EventHandler<String>`

```rust
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
    let mut pending_hotkey = use_signal(|| String::new());

    rsx! {
        Card { title: "Recording".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Hotkey" }
                button {
                    class: if *listening.read() { "hotkey-btn listening" } else { "hotkey-btn" },
                    tabindex: 0,
                    onclick: move |_| {
                        listening.set(true);
                        pending_hotkey.set(String::new());
                    },
                    onkeydown: move |e: Event<KeyboardData>| {
                        if !*listening.read() { return; }
                        e.prevent_default();

                        let key = e.key();

                        // Escape cancels
                        if key == Key::Escape {
                            listening.set(false);
                            return;
                        }

                        // Ignore bare modifier presses
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
        Key::F1 => "F1".into(),
        Key::F2 => "F2".into(),
        Key::F3 => "F3".into(),
        Key::F4 => "F4".into(),
        Key::F5 => "F5".into(),
        Key::F6 => "F6".into(),
        Key::F7 => "F7".into(),
        Key::F8 => "F8".into(),
        Key::F9 => "F9".into(),
        Key::F10 => "F10".into(),
        Key::F11 => "F11".into(),
        Key::F12 => "F12".into(),
        Key::End => "End".into(),
        Key::Home => "Home".into(),
        Key::Insert => "Insert".into(),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        _ => format!("{:?}", key),
    }
}
```

**Step 2: Add hotkey-btn CSS to styles.css**

Append after `.hotkey-display` block (~line 594):

```css
.hotkey-btn {
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-size: 12px;
  padding: 2px 8px;
  border: 2px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg-secondary);
  background: var(--bg-recessed);
  font-variant-numeric: tabular-nums;
  cursor: pointer;
  outline: none;
  min-width: 100px;
  text-align: center;
}

.hotkey-btn:hover {
  border-color: var(--accent);
}

.hotkey-btn.listening {
  border-color: var(--accent);
  color: var(--accent);
  animation: pulse-border 1s ease-in-out infinite;
}

@keyframes pulse-border {
  0%, 100% { border-color: var(--accent); }
  50% { border-color: var(--border); }
}
```

**Step 3: Update SettingsPage to wire up on_hotkey_change**

In `src/ui/settings/mod.rs`, add the new prop to `RecordingCard`:

```rust
RecordingCard {
    hotkey: config.read().recording.hotkey.clone(),
    mode: config.read().recording.mode.clone(),
    on_hotkey_change: move |hotkey: String| {
        config.write().recording.hotkey = hotkey;
    },
    on_mode_change: move |mode: String| {
        config.write().recording.mode = mode;
    },
}
```

**Step 4: Build and verify**

Run: `cargo build`
Expected: Compiles. Clicking the hotkey button enters listening mode, pressing a combo updates it.

**Step 5: Commit**

```bash
git add src/ui/settings/recording_card.rs src/ui/settings/mod.rs assets/styles.css
git commit -m "feat: add press-to-record hotkey picker in settings"
```

---

### Task 3: Re-register Hotkey on Save

**Files:**
- Modify: `src/ui/app.rs:85-111`
- Modify: `src/ui/settings/mod.rs` (save handler)

**Step 1: Store ShortcutHandle in a signal so we can remove it later**

In `src/ui/app.rs`, change the hotkey registration to store the handle in a signal, and make it reactive to config changes on save. Use `use_window().create_shortcut()` and `use_window().remove_shortcut()` directly instead of `use_global_shortcut()`.

Replace lines 85-111 in app.rs:

```rust
// Register global hotkey — store handle so we can swap it on save
let window_for_shortcut = window.clone();
let mut shortcut_handle: Signal<Option<ShortcutHandle>> = use_signal(|| None);

// Effect that registers/re-registers the hotkey whenever config changes
use_effect(move || {
    let cfg = config.read();
    let hotkey_str = cfg.recording.hotkey.clone();
    let is_toggle = cfg.recording.mode == "toggle";
    drop(cfg);

    // Remove old shortcut if any
    if let Some(old) = shortcut_handle.write().take() {
        window_for_shortcut.remove_shortcut(old);
    }

    let mut toggled = false;
    match window_for_shortcut.create_shortcut(hotkey_str.as_str(), move |state| {
        if is_toggle {
            if state == HotKeyState::Pressed {
                toggled = !toggled;
                if toggled {
                    coroutine.send(HotkeyEvent::RecordStart);
                } else {
                    coroutine.send(HotkeyEvent::RecordStop);
                }
            }
        } else {
            match state {
                HotKeyState::Pressed => coroutine.send(HotkeyEvent::RecordStart),
                HotKeyState::Released => coroutine.send(HotkeyEvent::RecordStop),
            }
        }
    }) {
        Ok(handle) => {
            shortcut_handle.set(Some(handle));
        }
        Err(e) => {
            tracing::error!("Failed to register hotkey '{}': {:?}", hotkey_str, e);
        }
    }
});
```

Also add the import at the top of app.rs:
```rust
use dioxus::desktop::ShortcutHandle;
```

**Step 2: Build and verify**

Run: `cargo build`
Expected: Compiles. Changing hotkey in settings and clicking Save re-registers the new hotkey.

**Step 3: Commit**

```bash
git add src/ui/app.rs
git commit -m "feat: re-register hotkey at runtime when settings are saved"
```

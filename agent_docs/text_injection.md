# Text Injection — UIA Fallback Chain

## Overview

Text injection is the most critical subsystem. The goal: transcribed text appears exactly where the user's cursor is, regardless of which application is focused.

## Fallback Chain (in order)

### 1. UIA SetValue (preferred)
- Get focused element via `IUIAutomation::GetFocusedElement()`
- Query for `IValueProvider` pattern
- Call `SetValue(text)` — atomically sets text
- **Pros:** Clean, fast, respects undo history in some apps
- **Cons:** Not all controls support `IValueProvider`

### 2. UIA SendKeys
- Get focused element, verify it supports text input
- Use `ITextPattern` if available to get insertion point
- Fall back to `SendKeys` via UI Automation
- **Cons:** Slower, may trigger autocomplete/suggestions

### 3. Win32 SendInput
- Convert each character to `INPUT` structs with `KEYEVENTF_UNICODE`
- Handle Unicode surrogate pairs for emoji/CJK
- Call `SendInput()` with array of inputs
- **Pros:** Works in most apps, handles Unicode
- **Cons:** May be blocked by some security software, triggers key event handlers

### 4. Clipboard Paste (last resort)
- Save current clipboard contents
- Set clipboard to transcribed text
- Simulate `Ctrl+V` via SendInput
- Wait 500ms, restore original clipboard
- **Pros:** Universally works
- **Cons:** Destructive to clipboard, timing-sensitive

## COM Threading

All UIA calls MUST happen on a thread with COM initialized:
```rust
tokio::task::spawn_blocking(|| {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok();
    }
    // ... UIA calls here ...
    unsafe { CoUninitialize(); }
})
```

## Per-App Workarounds

| App | Issue | Workaround |
|-----|-------|------------|
| VS Code | Electron, UIA partially works | SendInput preferred |
| Chrome address bar | UIA SetValue works | Use UIA |
| Chrome textarea | IValueProvider not available | SendInput or clipboard |
| Discord | Electron, custom input | SendInput |
| Word | Rich text, UIA works well | UIA SetValue |
| Notepad | Standard Win32 edit | UIA SetValue works |

## Debug Logging

Log every injection attempt:
- Target app name (from window title / process name)
- Method attempted and result (success/fail)
- Fallback chain traversal
- Final method that succeeded

## Control Type Detection

Use `IUIAutomationElement::CurrentControlType` to identify the focused control:
- `UIA_EditControlTypeId` — standard text input
- `UIA_DocumentControlTypeId` — rich text (Word, etc.)
- `UIA_ComboBoxControlTypeId` — dropdown with text input
- `UIA_CustomControlTypeId` — custom controls (Electron apps)

## Linux Fallback Chain

Linux uses a different chain. Target: GNOME 50 / Wayland on Ubuntu 26.04+.

Chain (in order): **ydotool → clipboard**

### 1. ydotool (keystroke synthesis via uinput)
- `ydotool type` for direct character injection. The clipboard backend also uses `ydotool key 29:1 47:1 47:0 29:0` for Ctrl+V or `29:1 42:1 47:1 47:0 42:0 29:0` for Ctrl+Shift+V.
- Writes directly to `/dev/uinput`, which is **kernel-level input injection**. This bypasses Xwayland and its `-enable-ei-portal` XTEST-forwarding entirely — no Remote Desktop prompt.
- Synthesizes the transcript character-by-character via uinput, so it works regardless of the app's paste shortcut convention or accessibility support (terminals, canvas apps, custom widgets — all fine).
- Requires the `ydotoold` daemon to be running and the user to be in the `input` group (same group the evdev hotkey listener already needs).
- Only handles ASCII — the backend transliterates common smart punctuation (`' ' " " – — …` → ASCII) but non-ASCII falls through to clipboard.

### 2. Clipboard (fallback)
- Sets the system clipboard via `arboard`, with `wl-copy` as a fallback writer and `wl-paste --no-newline` as a verifier (catches cases where arboard silently no-ops on Wayland).
- After setting, sends Ctrl+Shift+V (or Ctrl+V if `injection.paste_shortcut = "ctrl_v"`) via ydotool. The default is Ctrl+Shift+V because it covers every terminal and pastes-as-plain-text in most other apps — which is what you want for a transcript. If ydotool is unavailable, logs a hint and leaves the text on the clipboard for the user to paste manually.
- The previous clipboard is only restored when the paste actually fires — otherwise the transcript would get clobbered before the user could paste it.
- Mainly used for transcripts too long to type comfortably or containing non-ASCII text that ydotool-type can't handle.

### Focus-aware paste-shortcut detection (GNOME)

On GNOME Wayland, the compositor does not expose focused-window metadata to
unprivileged clients. To pick Ctrl+V vs Ctrl+Shift+V per-app, Beamer bundles
a minimal GNOME Shell extension (`extension/beamer-focus@beamer.app/`) that
exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` on the session bus
via the `org.gnome.Shell` name.

The Rust side (`src/injection/focus.rs`) queries this method on each paste
via zbus and classifies the returned app id against a curated terminal
list. Terminals → Ctrl+Shift+V; anything else → Ctrl+V. Any failure
(extension missing, disabled, D-Bus timeout, unknown app) falls back to
Ctrl+Shift+V — the current pre-feature default — so users who never
install the extension see no regression.

**Install:** Settings → Text Injection → "Install GNOME focus helper".
Button copies the extension files to
`~/.local/share/gnome-shell/extensions/beamer-focus@beamer.app/` and runs
`gnome-extensions enable`. GNOME renders its own native "enable extension?"
dialog — that's the privilege-grant moment.

**After install on Wayland**, the user must log out and back in once.
GNOME Shell does not hot-load new extensions on Wayland. The Settings card
hints at this when it detects the extension installed-but-not-yet-loaded
state.

**Terminal list** lives at `src/injection/focus.rs::TERMINAL_APP_IDS`.
To add a terminal, PR-append the app id (lowercase, exact match — no
substring heuristic).

**Supported GNOME versions:** 48, 49, 50. Older versions fall through the
feature-detection guard (`XDG_CURRENT_DESKTOP` + `XDG_SESSION_TYPE`) and
get the pre-feature default Ctrl+Shift+V. Non-GNOME desktops (KDE, wlroots)
same treatment — adding support per-compositor is a future item.

### What was removed and why

Earlier versions tried `dotool`, `wtype`, `enigo`, and an AT-SPI accessibility backend. All four are gone:
- `dotool` — niche, duplicates ydotool's uinput path.
- `wtype` — relies on the wlroots `zwp_virtual_keyboard_manager_v1` protocol, which GNOME Mutter does not implement. Always fails on GNOME.
- `enigo` — on Wayland the x11rb/XTEST backend is what Xwayland's `-enable-ei-portal` flag was specifically added to mediate. Using enigo reliably triggers the "Enable remote interaction" Remote Desktop dialog on Ubuntu 26.04+.
- `atspi` — D-Bus call into `org.a11y.atspi.EditableText.InsertText`. In principle it offered a semantic insert (correct cursor/undo behaviour) but in practice the registry walk broke against current `at-spi2-core` with a `Signature mismatch: got 'a(so)', expected 'a(sv)'` on `Registry.GetChildren`, and the wayland terminals we care about (Warp, Kitty, Alacritty, foot) don't register with AT-SPI at all. ydotool-type covers every app without the complexity.

### System setup

- `sudo usermod -aG input $USER` (required anyway for the evdev hotkey listener)
- `sudo apt install ydotool wl-clipboard` (or distro equivalent)
- Enable + start `ydotoold` as a user service (`systemctl --user enable --now ydotoold`) or system service
- No portal permissions, no accessibility toggles, no desktop files required.

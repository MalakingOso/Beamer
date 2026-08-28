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

## UIPI: dictation is dead against elevated windows, permanently

Beamer's manifest requests `asInvoker` (`build.rs`), so it always runs at the
same integrity level as the user's normal session. Windows' User Interface
Privilege Isolation blocks lower-integrity processes from sending input to a
higher-integrity window: an unelevated `WH_KEYBOARD_LL` hook never receives
key events destined for an elevated window, and `SendInput` into one is
silently dropped. Practically, dictating into Task Manager, an elevated
Command Prompt, or any "Run as administrator" window does nothing, and there
is no error to catch. The only fix is a signed binary carrying
`uiAccess="true"` and installed under Program Files, which is a code-signing
and installer commitment well beyond what this app does today. Unverified at
runtime (no Windows machine has run this build yet), but the mechanism is
Windows platform behaviour, not something Beamer's code could get right or
wrong.

`injection.paste_shortcut` (Ctrl+V vs Ctrl+Shift+V, see below) is a
Linux-only setting. On Windows the clipboard fallback in
`src/injection/clipboard.rs:282` hardcodes Ctrl+V and does not read the
config at all: there is no terminal-detection problem to solve there, since
Windows terminals accept Ctrl+V.

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

Linux uses a different chain. Target: GNOME 50 / Wayland on Ubuntu 26.04+,
with wlroots compositors (Sway/Hyprland/niri/COSMIC) and X11 covered by the
same chain's fallthrough behavior.

Chain (in order): **gnome → wtype → ydotool → clipboard**

All keystroke backends run the transcript through
`injection::sanitize_for_typing()` first: smart punctuation is
transliterated and newlines/tabs become spaces, so a typed Enter can never
submit a chat box or form mid-injection. The clipboard path keeps newlines
(an atomic paste doesn't press keys).

### 1. gnome (direct typing via the bundled Shell extension — preferred)
- The extension (v4; capability floor is v2) owns a
  `Clutter.VirtualInputDevice` inside GNOME Shell —
  the same mechanism as GNOME's on-screen keyboard — and exposes
  `TypeText(s) -> b` on `app.beamer.FocusProvider`.
- Full Unicode, layout-independent (keysym = codepoint | 0x01000000; Mutter
  remaps the keymap on demand), no uinput permissions, no portal dialogs, no
  clipboard involvement. This is the only first-class injection path on GNOME
  Wayland: Mutter implements neither `zwp_virtual_keyboard_v1` nor
  `ext-data-control-v1` nor `zwp_input_method_v2` (all explicitly refused
  upstream).
- Types ~8 chars per 16 ms Shell tick; the D-Bus reply arrives when typing
  finishes. Availability = `GetVersion() >= 2` (a v1 extension has no such
  method and maps to "unavailable").

### 2. wtype (Wayland virtual keyboard — wlroots compositors)
- `wtype -d 8 -- <text>` uses `zwp_virtual_keyboard_v1`: full Unicode via
  keymap upload, zero setup. The `-d 8` inter-key delay works around
  Chromium/Electron dropping full-speed events (known upstream bug).
- Availability probe: `wtype ""` binds the protocol without pressing keys —
  exits 0 on Sway/Hyprland/river/niri/labwc/COSMIC, exits 1 with "Compositor
  does not support the virtual keyboard protocol" on GNOME and KDE (both
  refuse the protocol intentionally, so this backend cleanly falls through).
- wtype was removed in April 2026 and reinstated in July 2026: the removal
  rationale was GNOME-specific, and the availability probe now prevents it
  from ever being tried there. The config migration no longer strips it.

### 3. ydotool (keystroke synthesis via uinput)
- `ydotool type --key-delay 25 --key-hold 20` for direct character injection.
- Writes to `/dev/uinput` — kernel-level, works on any compositor and X11.
- Requires the `ydotoold` daemon and `input`-group membership. Note: Ubuntu's
  `ydotool` package does not ship `ydotoold` units — users often need manual
  setup, which is why this backend is no longer first.
- ASCII only (post-sanitization non-ASCII bails to the next backend) and
  assumes a US layout — non-US layouts produce wrong characters, another
  reason `gnome`/`wtype` outrank it.

### 4. Clipboard (last resort)
- Sets the clipboard via `arboard` (falls back to X11/XWayland bridging on
  GNOME, where wl-copy needs a focus-hack), with `wl-copy` as a fallback
  writer and `wl-paste --no-newline` as a verifier. The verifier runs under a
  500 ms hard timeout — on GNOME wl-paste can hang in its transient-surface
  focus hack.
- Waits 150 ms for the compositor to advertise the offer, then sends the
  paste chord through the first working mechanism:
  **GNOME helper `SendPasteChord` → ydotool `key` → wtype `-M ctrl -k v`**.
- Chord choice (Ctrl+V vs Ctrl+Shift+V) via `resolve_use_shift_v`:
  `BEAMER_PASTE_SHORTCUT` env → `injection.paste_shortcut` config → "auto"
  (focus-helper terminal detection, defaulting to Ctrl+Shift+V when unknown).
- The previous clipboard is restored 500 ms after a successful chord — never
  earlier (apps read the offer lazily; restoring too soon pastes the OLD
  content), and never when the chord failed (that would clobber the
  transcript before the user could paste it).
- **When every chord mechanism fails, the text stays on the clipboard and a
  desktop notification tells the user to paste manually.** No silent no-ops.

### Focus-aware paste-shortcut detection (GNOME)

On GNOME Wayland, the compositor does not expose focused-window metadata to
unprivileged clients. To pick Ctrl+V vs Ctrl+Shift+V per-app, the bundled
extension exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` on the
session bus via the `org.gnome.Shell` name.

The Rust side (`src/injection/focus.rs`) queries this method on each paste
via zbus and classifies the returned app id against a curated terminal
list. Terminals → Ctrl+Shift+V; anything else → Ctrl+V. Any failure
(extension missing, disabled, D-Bus timeout, unknown app) falls back to
Ctrl+Shift+V.

**Terminal list** lives at `src/injection/focus.rs::TERMINAL_APP_IDS`.
To add a terminal, PR-append the app id (lowercase, exact match — no
substring heuristic).

### GNOME Shell extension v4 (`extension/beamer-focus@beamer.app/`)

D-Bus interface on `org.gnome.Shell` / `/app/beamer/FocusProvider`:

| Method | Purpose |
|---|---|
| `GetFocusedAppId() -> s` | focused app id (v1) |
| `GetVersion() -> u` | capability probe (returns 4) |
| `TypeText(s) -> b` | type Unicode text via virtual keyboard |
| `SendPasteChord(b) -> b` | Ctrl(+Shift)+V for the clipboard backend |
| `ShowIndicator(s)` / `UpdateLevel(d)` / `HideIndicator()` | shell-native recording pill (Deploy Purple waveform, bottom-center of the focused window's monitor, click-through) |

`REQUIRED_VERSION` in `src/injection/gnome.rs` is still **2** — that's the
version that introduced `TypeText`/`SendPasteChord`, and it's the real
capability floor. `HELPER_VERSION`/`metadata.json` are bumped past it purely
so existing installs surface as "update available" and pick up fixes; don't
raise `REQUIRED_VERSION` unless a genuinely new method is being depended on.

`TypeText` is served by `TypeTextAsync`, which types in batches on a GLib
timeout and replies when the last batch lands. The pending `invocation` is
parked in `this._typeInvocation` and answered exactly once via
`_finishTyping(ok)` — including from `disable()`, so tearing the extension
down mid-injection replies `false` immediately instead of stranding the
caller until its client-side timeout (seconds, scaled to text length) fires.

Rust clients: `src/injection/focus.rs` (focus), `src/injection/gnome.rs`
(typing + chord), `src/ui/shell_indicator.rs` (pill, dedicated worker
thread with coalesced level updates at ~15 Hz from `audio::subscribe_levels`).

**Install/update:** Settings → Text Injection. The card compares the live
version (`GetVersion`), installed metadata, and bundled metadata to offer
Install / Enable / Update, and tells the user to log out and back in when
Shell is still running old code (Wayland never hot-reloads extension JS).

**Supported GNOME versions:** 48, 49, 50. Non-GNOME desktops skip the
`gnome` backend via its availability probe.

### What was removed and why

Earlier versions tried `dotool`, `enigo`, and an AT-SPI accessibility backend. All are gone (wtype was also removed then, but is back — see above):
- `dotool` — niche, duplicates ydotool's uinput path.
- `enigo` — on Wayland the x11rb/XTEST backend is what Xwayland's `-enable-ei-portal` flag was specifically added to mediate. Using enigo reliably triggers the "Enable remote interaction" Remote Desktop dialog on Ubuntu 26.04+.
- `atspi` — D-Bus call into `org.a11y.atspi.EditableText.InsertText`. In principle it offered a semantic insert (correct cursor/undo behaviour) but in practice the registry walk broke against current `at-spi2-core` with a `Signature mismatch: got 'a(so)', expected 'a(sv)'` on `Registry.GetChildren`, and the wayland terminals we care about (Warp, Kitty, Alacritty, foot) don't register with AT-SPI at all.

Deliberately not (yet) added: the XDG RemoteDesktop portal / libei backend
(the sanctioned path for KDE and extension-less GNOME). It needs an async
portal session, an authorization dialog, and restore-token persistence that
is still flaky in the wild; the copy+notify degradation covers those setups
meanwhile. See docs/superpowers/specs/2026-07-18-linux-injection-v2-design.md.

### System setup

- **GNOME (recommended path):** install the helper extension from Settings,
  log out/in once. Nothing else — no groups, no daemons, no portals.
- **wlroots compositors:** `apt install wtype` (or distro equivalent). Nothing else.
- **Fallback/legacy:** `sudo usermod -aG input $USER`, `apt install ydotool
  wl-clipboard`, enable `ydotoold` as a user or system service.

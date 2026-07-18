# Linux Injection v2 + Shell-Native Recording Pill — Design

Date: 2026-07-18
Status: Approved (autonomous session — user requested "always pastes properly on
Linux" + VibeTyper-style recording visuals, with research into voquill,
hyprvoice, and VibeTyper)

## Problem

Beamer's Linux injection chain is `ydotool → clipboard`. Both legs funnel
through ydotool (`type` for direct typing, `key` for the paste chord), which
fails in real contexts:

- `ydotool type` is ASCII-only and assumes a US QWERTY keymap. Any non-ASCII
  character (beyond the transliteration table) aborts the whole backend.
- Ubuntu packages ydotool without guaranteeing a running `ydotoold`; when the
  daemon is absent the chord silently fails and text is left on the clipboard
  with **no user-visible notification**.
- GNOME/Mutter (the primary target: GNOME 50 Wayland) supports **none** of the
  Wayland injection protocols: no `zwp_virtual_keyboard_v1` (wtype), no
  `ext-data-control-v1` (wl-copy/wl-paste run through a focus-hack), no
  `zwp_input_method_v2`. `wl-paste`-based clipboard verification is therefore
  unreliable on GNOME.
- The recording pill is disabled on Linux (tray-icon swap instead) because
  Wayland toplevels can't be positioned or made always-on-top.

## Research findings (what the referenced projects do)

- **voquill**: arboard(data-control) + 40 ms settle → ydotool v1 scancodes →
  wtype fallback → leave-on-clipboard final fallback; 800 ms delayed clipboard
  restore in a detached thread.
- **hyprvoice**: ordered exec of `ydotool type` → `wtype` → `wl-copy`
  (copy-only, no chord). Fails on GNOME/KDE precisely because both tools do.
- **VibeTyper** (binary inspected): Electron; own uinput virtual keyboard via
  udev rule; wl-clipboard + simulated chord with clipboard snapshot/restore;
  app-aware chord table (terminals → Ctrl+Shift+V, unknown Wayland windows →
  Shift+Insert); degrades to "Copied. Press Ctrl+V" notification. Overlay is a
  dark glass pill, bottom-center, waveform + timer, because Electron windows
  run under XWayland and can be positioned.
- **Landscape**: a GNOME Shell extension can inject arbitrary-keysym key events
  via `Clutter.VirtualInputDevice` (the same path as GNOME's on-screen
  keyboard) — full Unicode, layout-independent, zero permissions/dialogs. This
  is the only first-class injection mechanism on GNOME, and Beamer already
  ships a GNOME extension (focus helper v1).

## Design

### 1. Extension v2: `beamer-focus@beamer.app` grows typing + indicator

D-Bus interface `app.beamer.FocusProvider` (path unchanged) gains:

| Method | Signature | Purpose |
|---|---|---|
| `GetFocusedAppId` | `() → s` | unchanged (v1) |
| `GetVersion` | `() → u` | capability probe; returns 2 |
| `TypeText` | `(s) → b` | type full Unicode text via `Clutter.VirtualInputDevice`, paced in small batches |
| `SendPasteChord` | `(b use_shift) → b` | press Ctrl(+Shift)+V for the clipboard backend |
| `ShowIndicator` | `(s state)` | show the shell-native pill ("recording" \| "processing") |
| `UpdateLevel` | `(d)` | live mic level 0.0–1.0 for the waveform |
| `HideIndicator` | `()` | hide the pill |

Typing maps each codepoint to a keysym (`cp < 0x100 ? cp : cp | 0x01000000` —
the X11 Unicode rule Mutter accepts, remapping the keymap on demand) and sends
press/release batches of ~8 chars per 16 ms tick with an async D-Bus return.
Files stay flat (`extension.js`, `indicator.js`, `stylesheet.css`,
`metadata.json`) because the installer copies top-level files only.

The indicator is an `St.BoxLayout` added via `Main.layoutManager.addTopChrome`
(click-through, never focusable, bottom-center of the primary monitor) — the
VibeTyper look: 48 px dark glass capsule (near-black gradient, 1 px white
hairline ring, soft shadow, fully rounded), animated waveform bars tinted
across a purple gradient (#4B0082 → #A561EC), white elapsed timer in
monospace, "Transcribing…" label in the processing state, scale/slide entrance
and exit animations.

### 2. Rust backend lineup (Linux default order)

`["gnome", "wtype", "ydotool", "clipboard"]`

- **`gnome` (new)** — calls `TypeText` over D-Bus (blocking zbus, timeout
  scaled to text length). Available when the extension answers `GetVersion ≥ 2`.
  Fully replaces clipboard use on GNOME for normal dictation.
- **`wtype` (new)** — `wtype -d 8 -- <text>` for wlroots/COSMIC/niri (full
  Unicode via keymap upload; `-d 8` avoids Chromium's dropped-event bug).
  Availability probe: `wtype ""` exits 0 only where the compositor implements
  virtual-keyboard-v1 (fails instantly on GNOME/KDE).
- **`ydotool`** — unchanged behavior, now third.
- **`clipboard`** — improved:
  - chord senders in order: extension `SendPasteChord` → ydotool → wtype
    (`wtype -M ctrl [-M shift] -k v …`);
  - clipboard-settle delay 80 ms → 150 ms (between espanso's 300 and
    voquill's 40; Handy's 60 is known-flaky);
  - `wl-paste` verification gets a hard timeout so GNOME's focus-hack path
    can't hang the injection;
  - when no chord mechanism works, the existing "text left on clipboard"
    path now fires a **desktop notification** ("Copied to clipboard — press
    Ctrl+V to paste") instead of only logging.

Shared `sanitize_for_typing()` (mod.rs): transliterate smart punctuation
(moved from ydotool.rs) and map newlines/tabs to spaces so typing backends
can never press Enter mid-injection (submitting chat messages/forms — the
hyprvoice issue #25 lesson). The clipboard path keeps newlines (an atomic
paste doesn't submit).

Config migration: `wtype` is removed from the `REMOVED` backends list (it was
purged for GNOME-specific reasons that no longer apply since it now sits
behind the availability probe), and stored chains equal to the legacy default
`["ydotool", "clipboard"]` upgrade to the new default.

### 3. Live mic levels

The audio conversion thread (audio/mod.rs) computes per-chunk RMS and
publishes `min(1.0, rms_normalized * 8)` to a global `tokio::sync::watch`
channel. Consumers: the GNOME indicator pump (D-Bus `UpdateLevel` at ~15 Hz
while recording) — and nothing else for now; the Windows/macOS pill keeps
CSS-animated bars.

### 4. UI

- **Linux/GNOME**: app.rs drives the shell indicator from `rec_state`
  (show/hide/state), keeping the tray-icon swap for non-GNOME desktops and as
  redundant feedback. Fire-and-forget zbus calls; a cached availability probe
  avoids D-Bus spam when the extension is absent.
- **Windows/macOS pill**: restyled to the same VibeTyper language (dark glass
  capsule, purple-gradient bars, timer via a small JS interval, states
  Recording → waveform+timer, Processing → "Transcribing…"). DOM/wiring
  unchanged to keep risk low on platforms this session can't compile.
- **Settings**: injection card lists the new backends automatically; the
  helper row becomes version-aware — live version via `GetVersion`, bundled
  version from metadata; states gain "Update available" (button reinstalls)
  and "Updated — log out and back in".

### 5. Error handling / guarantees

Every path ends in something visible: direct typing → typed text; chord paste
→ pasted text; degraded → clipboard + notification; total failure →
notification (existing orchestrator fallback). No silent no-ops.

### Out of scope (documented for later)

- RemoteDesktop portal backend (ashpd/libei) for KDE-without-ydotool and
  extension-less GNOME — sanctioned but heavy (async portal session, dialog
  UX, flaky persistence per Deskflow bounty); the copy+notify degradation
  covers those setups meanwhile.
- Shift+Insert chord + PRIMARY selection for Konsole/Electron.
- Layer-shell pill for wlroots desktops.
- Live-level waveform in the Windows/macOS pill.

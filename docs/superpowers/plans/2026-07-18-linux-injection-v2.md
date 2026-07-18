# Linux Injection v2 + Shell-Native Pill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Linux text injection reliable everywhere (GNOME Wayland first-class via a Shell-extension typing backend, wlroots via wtype, hardened clipboard fallback that never fails silently) and adopt VibeTyper-style recording visuals (shell-native pill on GNOME, restyled pill on Windows/macOS).

**Architecture:** Extend the bundled GNOME extension to v2 (D-Bus typing via `Clutter.VirtualInputDevice`, paste chord, St-widget indicator pill). Add `gnome` and `wtype` injection backends ahead of `ydotool`; the clipboard backend gains a chord-mechanism chain and a copy+notify degradation. Live mic levels flow through a global watch channel to the indicator.

**Tech Stack:** Rust (zbus 5 blocking, tokio watch), GJS/Clutter/St (GNOME 48–50), wtype/ydotool subprocesses, Dioxus 0.7 webview CSS.

Spec: `docs/superpowers/specs/2026-07-18-linux-injection-v2-design.md`

---

### Task 1: Shared typing sanitizer

**Files:** Modify `src/injection/mod.rs`, `src/injection/ydotool.rs`

- [ ] Add `pub(crate) fn sanitize_for_typing(text: &str) -> String` to `mod.rs`: transliterate smart punctuation (move table from ydotool.rs) then map `\r\n`/`\r`/`\n`/`\t` → single space, trim end. Unit tests: smart quotes → ascii, newline → space (no Enter), CRLF collapses to one space, interior double spaces preserved.
- [ ] ydotool.rs uses `sanitize_for_typing` then keeps its ASCII bail.
- [ ] `cargo test` green. Commit `feat(injection): shared typing sanitizer strips newlines`.

### Task 2: Extension v2 — D-Bus typing + chord

**Files:** Modify `extension/beamer-focus@beamer.app/extension.js`, `extension/beamer-focus@beamer.app/metadata.json`

- [ ] metadata: `"version": 2`, description mentions typing + indicator.
- [ ] extension.js: keep `GetFocusedAppId`; add `GetVersion() → 2`; create `Clutter.VirtualInputDevice` (KEYBOARD) lazily from `Clutter.get_default_backend().get_default_seat()`; `TypeText(s) → b` async D-Bus impl typing 8 chars per 16 ms `GLib.timeout_add` tick, keysym = `cp < 0x100 ? cp : cp | 0x01000000`, press+release with `GLib.get_monotonic_time()`; `SendPasteChord(b) → b` pressing Ctrl(+Shift)+V keysyms (0xffe3/0xffe1/0x76). Reject overlapping TypeText calls. Destroy device + remove timeouts in `disable()`.
- [ ] Syntax check: `cp extension.js /tmp/ext-check.mjs && node --check /tmp/ext-check.mjs`.
- [ ] Commit `feat(extension): v2 — virtual-keyboard typing and paste chord over D-Bus`.

### Task 3: Extension v2 — indicator pill

**Files:** Create `extension/beamer-focus@beamer.app/indicator.js`, `extension/beamer-focus@beamer.app/stylesheet.css`; modify `extension.js`

- [ ] indicator.js: `BeamerIndicator` class — `St.BoxLayout` pill added with `Main.layoutManager.addTopChrome(pill, {affectsInputRegion: false})`, positioned bottom-center of primary monitor (re-positioned on `monitors-changed`); children: 12 waveform bars (3 px wide, per-bar color interpolated #4B0082→#A561EC), timer `St.Label` ("0:00", ~1 Hz update), status label for "Transcribing…". 30 fps bar animation: per-bar phase offset sine modulated by latest level (idle breathing floor 0.15). `show(state)`, `setLevel(l)`, `hide()` with ease() scale/translate entrance/exit. All timers removed on hide/destroy.
- [ ] stylesheet.css: `.beamer-pill` dark glass capsule (48 px, radius 24 px, `rgba(16,16,18,0.92)` bg, 1 px `rgba(255,255,255,0.14)` border, shadow), `.beamer-pill-timer` / `.beamer-pill-status` white monospace.
- [ ] extension.js: import indicator; D-Bus methods `ShowIndicator(s)`, `UpdateLevel(d)`, `HideIndicator()`; destroy in `disable()`.
- [ ] `node --check` both JS files. Commit `feat(extension): shell-native recording pill (waveform + timer)`.

### Task 4: `gnome` backend + focus.rs proxy reuse

**Files:** Modify `src/injection/focus.rs`; create `src/injection/gnome.rs`; modify `src/injection/mod.rs`

- [ ] focus.rs: extract `pub(crate) fn helper_proxy(timeout_ms: u64) -> Result<zbus::blocking::Proxy<'static>, zbus::Error>`; keep `focused_app_id()` on top of it.
- [ ] gnome.rs (`#![cfg(not(target_os = "windows"))]`): `helper_version() -> Option<u32>` (`GetVersion`, 100 ms timeout); `GnomeBackend` — `name() = "gnome"`, available = version ≥ 2 else instructive error ("helper v2 not active — install/update in Settings, then log out/in"), inject = sanitize → `TypeText` with timeout `2000 + 20·len` ms, error if reply false. Also `pub(crate) fn send_paste_chord(use_shift: bool) -> bool` for the clipboard backend (500 ms timeout).
- [ ] mod.rs: register `GnomeBackend` first on non-Windows.
- [ ] `cargo build` green. Commit `feat(injection): gnome backend — direct typing via helper extension`.

### Task 5: `wtype` backend

**Files:** Create `src/injection/wtype.rs`; modify `src/injection/mod.rs`

- [ ] wtype.rs: available = `WAYLAND_DISPLAY` set, `wtype` on PATH, probe `wtype ""` exit 0 (fails fast on GNOME/KDE: "compositor does not support the virtual keyboard protocol"); inject = `wtype -d 8 -- <sanitized>`; `pub(crate) fn send_paste_chord(use_shift: bool) -> bool` (`wtype -M ctrl [-M shift] -k v [-m shift] -m ctrl`).
- [ ] mod.rs: register after `gnome`, before `ydotool`.
- [ ] Commit `feat(injection): wtype backend for wlroots compositors`.

### Task 6: Clipboard hardening

**Files:** Modify `src/injection/clipboard.rs`

- [ ] Chord chain `try_paste_chord()`: gnome (if `helper_version() ≥ 2`) → ydotool (existing) → wtype; returns `Option<&'static str>` (mechanism) for `target_info`.
- [ ] Settle delay 80 → 150 ms. `verify_clipboard_contains` runs `wl-paste` under a 500 ms timeout (spawn + poll + kill) so GNOME's focus-hack can't hang.
- [ ] Manual-paste degradation (`Ok(false)` path) fires a `notify_rust` notification "Copied to clipboard — press Ctrl+V to paste" (Linux cfg).
- [ ] Keep `choose_use_shift_v` tests green; add test that chord-mechanism order lists gnome first (pure ordering fn).
- [ ] Commit `fix(injection): clipboard — chord chain, verify timeout, visible manual-paste fallback`.

### Task 7: Defaults + config migration

**Files:** Modify `src/injection/mod.rs`, `src/config/mod.rs`

- [ ] `default_backend_names()` (non-Windows) → `["gnome", "wtype", "ydotool", "clipboard"]`.
- [ ] config: remove `"wtype"` from `REMOVED`; migrate stored `["ydotool", "clipboard"]` → new defaults (dirty save). Tests: legacy default upgrades; custom chains untouched; wtype no longer stripped.
- [ ] `cargo test` green. Commit `feat(injection): new Linux default chain + config migration`.

### Task 8: Live mic levels

**Files:** Modify `src/audio/mod.rs`

- [ ] Global `watch::channel<f32>` in `OnceLock`; conversion thread computes RMS per chunk, publishes `min(1.0, (rms/32768)·8)`; `pub fn subscribe_levels() -> watch::Receiver<f32>`; publish 0.0 on stream end. Unit test for the normalization fn.
- [ ] Commit `feat(audio): publish live mic level for recording indicators`.

### Task 9: Shell indicator client + app wiring

**Files:** Create `src/ui/shell_indicator.rs`; modify `src/ui/mod.rs`, `src/ui/app.rs`

- [ ] shell_indicator.rs (non-Windows): cached helper-v2 probe (refreshed on `show`); `show(state: &str)`, `update_level(f32)`, `hide()` — fire-and-forget `spawn_blocking` zbus with short timeouts, no-ops when helper absent.
- [ ] app.rs Linux block: `use_effect` on `rec_state` → show("recording"/"processing")/hide (tray swap kept); one long-lived task pumping `subscribe_levels()` → `update_level` at ≥66 ms intervals while Recording.
- [ ] `cargo build` green. Commit `feat(ui): drive shell-native pill from recording state`.

### Task 10: Windows/macOS pill restyle

**Files:** Modify `src/ui/app.rs` (PILL_CSS + evaluate_script), `src/ui/pill.rs`

- [ ] pill.rs: add `span.pill-timer`; PILL_CSS → dark glass capsule (48 px, radius 9999, `#17171a→#101012` gradient, hairline ring, purple-gradient bars, white DM Mono text). Show-script starts a JS 1 s timer interval into `.pill-timer` when Recording (label hidden), shows "Transcribing…" when Processing (timer stops); hide-script clears interval.
- [ ] Careful review only (path not compiled on Linux). Commit `feat(ui): VibeTyper-style pill for Windows/macOS`.

### Task 11: Settings — helper update flow

**Files:** Modify `src/install/gnome_extension.rs`, `src/ui/settings/injection_card.rs`

- [ ] gnome_extension.rs: `bundled_version()`/`installed_version()` (serde_json parse of metadata.json), `live_version()` (D-Bus via focus helper proxy; missing method → 1); `Status` gains `UpdateAvailable` (installed < bundled) and `UpdatePendingRestart` (files current, live < bundled). `status()` checks these when Enabled. Tests for version compare logic.
- [ ] injection_card.rs: map new states — "Update helper for direct typing" [Update → `install()`], "Helper updated — log out and back in". Gate row on GNOME (Wayland or X11).
- [ ] `cargo test` green. Commit `feat(settings): version-aware GNOME helper update flow`.

### Task 12: Docs + final verification

**Files:** Modify `agent_docs/text_injection.md`, `agent_docs/config_schema.md`; spec/plan committed

- [ ] text_injection.md Linux section rewritten: new chain, extension v2 interface, chord chain, degradation guarantees, un-removed wtype rationale.
- [ ] config_schema.md: new default backends.
- [ ] `cargo build --release && cargo test`; `node --check` extension files; file sizes < 500 lines.
- [ ] Commit `docs: linux injection v2`.

**Deferred (spec "out of scope"):** portal backend, Shift+Insert/PRIMARY, layer-shell pill, live-level Windows pill.

# Beamer TODO

## Packaging / Release

- [ ] **Bundle GNOME extension in release artifacts** — No release pipeline exists yet (no CI, no build scripts, no `cargo-deb` config; `installer/` only has `.bmp` assets for a never-built Windows NSIS installer). When a release pipeline is created, copy `extension/beamer-focus@beamer.app/` into the artifact so `locate_source_dir()` can find it. Preferred placement (FHS fallback 2): `<artifact_root>/share/beamer/extension/beamer-focus@beamer.app/`. Acceptable fallback (portable fallback 3): `<artifact_root>/extension/beamer-focus@beamer.app/`. Dev/cargo-run fallback (fallback 4) already works because the directory sits at the repo root.

## Features
- [ ] ElevenLabs usage dashboard — API supports `GET /v1/user/subscription` (character_count/character_limit) and `GET /v1/usage/character-stats` (historical data with aggregation). Mistral has no usage API.
- [ ] Overlay window — the `Overlay` component exists but isn't wired to a separate transparent window for showing live transcription text on screen

## Sticky Notes — omissions

- [ ] **The Windows target does not compile, in more places than were written
      down.** Beyond `src/hotkey/ll_hook.rs:137,143` constructing
      `HotkeyEvent::RecordStart` with no payload, the module's whole signature
      drifted from its callers: `start_ll_hook` takes 2 params where `app.rs`
      passes 3, and `update_config` was never renamed to `update_configs`. More
      fundamentally `ll_hook.rs` has **no data structure that could hold a
      second binding** — no `BindingConfig`, no `Modifiers`, no
      `matching_binding`, just four loose bools. The Windows note hotkey is not
      unwired, it is unrepresentable. Roughly 100-150 mostly-mechanical lines,
      best fixed by hoisting the platform-neutral matching logic out of
      `linux_hotkey.rs:20-72` into `hotkey/mod.rs` so both platforms share
      tested code. Verifiable without a Windows machine:
      `cargo check --target x86_64-pc-windows-msvc` works here (target and deps
      installed, `check` never links).
- [ ] **`HotkeyConfig` cannot express Super as a modifier.** It has `ctrl`,
      `alt` and `shift` fields but no Super/Meta, and `parse()` only maps Super
      to a trigger key (`VK_LWIN`) when nothing else follows it. So
      `"Super+N"` parses to `ctrl/alt/shift = false, trigger_vk = 'N'` — the
      Super is silently dropped and the binding fires on a **bare N keypress**.
      Verified, not inferred. Any `Super+<key>` chord is a footgun; today the
      only safe Super chords are those where Super *is* the trigger
      (`Ctrl+Super`, `Ctrl+Alt+Super`). Either add a `win` field to
      `HotkeyConfig` and thread it through `Modifiers`/`matching_binding`, or
      reject `Super+<key>` in `parse()` so it returns `None` instead of a
      dangerous binding. The second is the smaller fix and fails safe —
      `note_hotkey_config()` already treats `None` as "unbound".
- [ ] `src/notes/task_store.rs` is **474 lines against the 500 limit**, about
      200 of it tests. The next addition needs the tests split out first.
- [ ] **Suggestion count badge on the notes board.** The spec (§9) calls for a
      note with pending suggestions to show a count on its card in the board.
      The Phase 2/3 plan did not ask for it and it was not built. Small: the
      board would need the `TaskStore` signal as a prop and
      `suggested_for(&id).len()`.
- [ ] Note windows are placed but their **size** is never captured, so resizing
      a note is forgotten on restart. `Note::size` is read and honoured; nothing
      writes it. (Position is forgotten *by design* — see
      `agent_docs/sticky_notes.md` — but size has no such justification.)
- [ ] `GetWorkArea` extension method. Placement insets a fixed 40px for the
      GNOME panel; a real work area would account for docks and other struts.
      Speculative, so deferred — and it costs only the log out that any other
      extension change costs anyway. `GetWindowFrame` already ships unused and
      is the verification path for it.
- [ ] **Phase 3's extraction prompt is untuned.** It was spot-checked against
      the live model (five probes, all correct, including aspirations phrased
      like commitments) but never measured against a corpus, because none
      existed. `cargo run --bin task_eval` grades it against your own
      accept/dismiss decisions; run it once a few dozen notes have accumulated.
      If precision is poor, the levers are the prompt and the model ladder —
      **not** `min_confidence`, which measured 0.90-0.98 across every probe and
      filters approximately nothing.
- [ ] `assets/styles.css:752` references `var(--bg-elevated)`, which is not
      defined in the token block. Pre-existing since `c709faa`, unrelated to
      notes; an undefined custom property fails silently.

## Low Priority
- [ ] `overlay_enabled` config field is never read — wire it to conditionally show/hide the glow overlay
- [ ] `debug_logging` toggle saves to config but has no runtime effect — consider wiring it to control injection trace logging
- [ ] Auto-start toggle in settings UI — `auto_start` config field exists but there's no settings control for it
- [ ] `set_auto_start(false)` code path is unreachable — no UI to disable auto-start once enabled

## Done
- [x] Quick fix: skip SendInput for Warp (process-name detection, route to clipboard)
- [x] Fix vocabulary persistence — terms now save immediately on add/remove instead of only on "Save Changes"
- [x] Fix hotkey picker — redesigned with modifier checkboxes (Ctrl/Alt/Shift/Win) + key capture button. No more freezing.
- [x] Fix screen edge glow — Glow component now renders inside main window when recording, with pulsing animation
- [x] Rename "Hold-to-talk" to "Push to Talk" everywhere in the UI
- [x] Add 400ms release delay after hold-to-talk — continues capturing audio briefly so last word isn't clipped
- [x] Remove acrylic backdrop — switched to solid backgrounds, removed DWM backdrop code and Win32_Graphics_Dwm feature
- [x] History copy button — now uses Phosphor Copy/Check icons with green feedback flash on copy
- [x] Fix settings window visual glitches — added overflow-x:hidden, min-width:0 on flex containers
- [x] API usage research — ElevenLabs has usage endpoints, Mistral does not
- [x] Remove dead HotkeyHandler code — stripped hotkey/mod.rs to just the HotkeyEvent enum, removed #![allow(dead_code)]
- [x] Deduplicate load_api_key/save_api_key — moved to config/mod.rs, removed copies from orchestrator.rs and settings/mod.rs
- [x] Clean up dead code — removed OverlayApp, GlowApp, AdvancedConfig, Vocabulary::import_from_file, unused CSS classes

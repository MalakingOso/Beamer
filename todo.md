# Beamer TODO

## Packaging / Release

- [ ] **Bundle GNOME extension in release artifacts** — No release pipeline exists yet (no CI, no build scripts, no `cargo-deb` config; `installer/` only has `.bmp` assets for a never-built Windows NSIS installer). When a release pipeline is created, copy `extension/beamer-focus@beamer.app/` into the artifact so `locate_source_dir()` can find it. Preferred placement (FHS fallback 2): `<artifact_root>/share/beamer/extension/beamer-focus@beamer.app/`. Acceptable fallback (portable fallback 3): `<artifact_root>/extension/beamer-focus@beamer.app/`. Dev/cargo-run fallback (fallback 4) already works because the directory sits at the repo root.

## Features
- [ ] ElevenLabs usage dashboard — API supports `GET /v1/user/subscription` (character_count/character_limit) and `GET /v1/usage/character-stats` (historical data with aggregation). Mistral has no usage API.
- [ ] Overlay window — the `Overlay` component exists but isn't wired to a separate transparent window for showing live transcription text on screen

## Sticky Notes — Phase 1 omissions

- [ ] **The Windows target does not compile.** `src/hotkey/ll_hook.rs:137,143`
      still construct `HotkeyEvent::RecordStart` with no payload; Task 2 gave the
      variant a `CaptureMode` and only fixed `linux_hotkey.rs`. The file is
      `#[cfg(target_os = "windows")]`, so Linux builds stay green and this is
      invisible here. **Task 4 is now a build fix, not parity work** — and until
      it lands the Windows note hotkey does not exist at all.
- [ ] Note windows are placed but their **size** is never captured, so resizing
      a note is forgotten on restart. `Note::size` is read and honoured; nothing
      writes it. (Position is forgotten *by design* — see
      `agent_docs/sticky_notes.md` — but size has no such justification.)
- [ ] `GetWorkArea` extension method. Placement insets a fixed 40px for the
      GNOME panel; a real work area would account for docks and other struts.
      Speculative, so deferred — and it costs only the log out that any other
      extension change costs anyway. `GetWindowFrame` already ships unused and
      is the verification path for it.
- [ ] `agent_docs/local_inference.md` — the spec calls for it; `src/llm/` is
      currently documented only in `config_schema.md` and `sticky_notes.md`.
- [ ] Phase 2 (S1-mini cleanup pass) and Phase 3 (task extraction) are
      unbuilt. Phase 2 is unblocked — the latency numbers were measured, not
      estimated. Phase 3 waits on an eval corpus from real Phase 1 use.
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

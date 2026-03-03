# Beamer TODO

## Features
- [ ] ElevenLabs usage dashboard — API supports `GET /v1/user/subscription` (character_count/character_limit) and `GET /v1/usage/character-stats` (historical data with aggregation). Mistral has no usage API.
- [ ] Overlay window — the `Overlay` component exists but isn't wired to a separate transparent window for showing live transcription text on screen

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

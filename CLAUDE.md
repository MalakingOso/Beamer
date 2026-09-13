# Beamer

Windows system-tray dictation app. Captures mic audio, transcribes via cloud APIs, injects text into the focused input field using a UI Automation fallback chain.

## Project Structure

- `src/main.rs` — Entry point, tray icon, Dioxus launch
- `src/audio/` — Mic capture (cpal) with inline downmix/resample + chunker thread (no VAD)
- `src/transcription/` — Backend trait + 4 implementations (ElevenLabs/Mistral, batch/realtime)
- `src/injection/` — Text injection fallback chain (UIA → SendInput → clipboard)
- `src/hotkey/` — Global hotkey registration (hold-to-talk + toggle modes)
- `src/config/` — TOML config + vocabulary management
- `src/notes/` — Note store (`mod.rs`) + types (`model.rs`), stage transitions
  (`lifecycle.rs`), size/delete edits (`edit.rs`), the model-pass
  coroutine (`pipeline.rs`), task suggestions (`task.rs`, `task_store.rs`),
  calendar export (`ics.rs`)
- `src/llm/` — Client for the standalone llama.cpp server (Beamer never spawns
  it). Task extraction (`extract.rs`) over `chat.rs`; `prompts.rs` holds the
  extraction prompt.
  ⚠️ **No crate-rooted paths in this directory** — `src/bin/task_eval.rs`
  `#[path]`-includes it, and there is no `src/lib.rs`.
- `src/tray/` — System tray icon + menu
- `src/ui/` — Dioxus desktop: settings window, overlay, screen edge glow
  - `src/ui/settings/` — One file per card section (recording, transcription, api_keys, etc.)
  - `src/ui/sticky*.rs`, `note_layout.rs`, `shell_window.rs` — Sticky note
    windows and placement
  - `src/ui/notes_page.rs` — All-notes board; `tasks_page.rs` — accepted tasks
  - `src/ui/components.rs` — Shared: Card, Select, Toggle, MaskedInput, TagChip

## Commands

```
dx build                       # Dev build
dx build --release             # Release (use for injection testing)
dx serve                       # Run with hot reload
dx run                         # Run without hot reload
RUST_LOG=beamer=debug dx serve   # Run with debug logging
cargo run --bin task_eval -- --limit 20   # Measure extraction against your own decisions
```

## Code Constraints

- Keep all files under 500 lines of code — split and refactor when approaching the limit
- Dioxus 0.7 owns the main thread and tokio runtime — never create a second runtime
- All UIA/Win32 calls MUST go through tokio::task::spawn_blocking() with COM initialized (sole exception: the low-level keyboard hook in `src/hotkey/ll_hook.rs`, which must own a real thread with a Windows message loop — `SetWindowsHookExW`/`GetMessageW` cannot run on the runtime's pool)
- tray-icon and muda used directly (not via Dioxus re-exports)
- API keys stored in Windows Credential Manager via keyring — never on disk
- Design language: Beamer Purple (purple accent, 2px borders, hard-offset shadows, solid backgrounds)

## Git & Commits

- Do not add "Co-Authored-By: Claude" footers to commits — only use when actually writing code alongside the user, not for commit messages alone

## Detailed Docs (read before working on related code)

- `agent_docs/text_injection.md` — UIA fallback chain, per-app workarounds (CRITICAL)
- `agent_docs/transcription_backends.md` — API contracts, WebSocket protocols, retry
- `agent_docs/audio_pipeline.md` — cpal setup, VAD, sample rates, buffer sizing
- `agent_docs/config_schema.md` — TOML schema, vocab format
- `agent_docs/dioxus_architecture.md` — Threading model, multi-window, tray integration
- `agent_docs/design_system.md` — Color tokens, typography, component patterns
- `agent_docs/sticky_notes.md` — Note windows, Wayland placement, cross-window state (CRITICAL for multi-window work)
- `agent_docs/local_inference.md` — The extraction pass, the pipeline coroutine, and the failures that return HTTP 200 (CRITICAL before touching `src/llm/`)
- `agent_docs/sync.md` — Cross-machine sync design (automerge doc, live sync client)
- `agent_docs/running_on_bearcave.md` — Deploying/running Beamer on the Windows laptop against callisto's models
- `docs/decisions.md` — The "why" behind major shipped features, one paragraph each

---

## Current status

Version 1.0.3 on `master`. Sticky Notes (all 3 phases) and cross-machine
sync are shipped; note attachments (files, images, link chips) and the
S1-mini cleanup pass have since been removed as dead weight — extraction
is now the only local model pass. The GNOME extension ships inside the
.deb and `deploy/install-linux.sh`, so Settings → Injection can install it
without a checkout; a GNOME log-out/in is still needed to pick up a new
extension version, but it's no longer a separate manual deploy step. The
feature set is large and growing — run `cargo test` (CI enforces it with
`-D warnings`) rather than trusting any count here.

The detailed docs are the source of truth:
- `agent_docs/sticky_notes.md` — Note windows, Wayland placement, cross-window state
- `agent_docs/local_inference.md` — The extraction pass, pipeline coroutine, and edge cases
- `agent_docs/sync.md` — Cross-machine sync design

`roadmap.md` tracks what's left (known issues + low-priority items); it
replaced `todo.md` once the larger parked items resolved.

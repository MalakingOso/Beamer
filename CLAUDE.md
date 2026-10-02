# Beamer

Windows system-tray dictation app. Captures mic audio, transcribes via cloud APIs, injects text into the focused input field through a per-platform fallback chain.

## Project Structure

- `src/main.rs` — Process setup only (logging, single instance, autostart,
  desktop identity), then Dioxus takes the main thread
- `src/orchestrator/` — The dictation conductor: hotkey → audio →
  transcription → `sink` (inject into the focused field, or capture a note).
  Capture (`mod.rs`) and a FIFO transcription worker (`transcribe.rs`) run
  side by side, so an upload never blocks the next recording. `session.rs`
  (tail capture), `notify.rs` (notifications)
- `src/audio/` — Mic capture (cpal) with inline downmix/resample + chunker thread (no VAD)
- `src/transcription/` — 3 batch backends: ElevenLabs Scribe v2, Scribe v2 medical, Mistral Voxtral;
  shared client, timeouts and retry rule in `mod.rs`
- `src/injection/` — Text injection fallback chain (Windows: SendInput →
  clipboard → UIA; Linux: gnome → wtype → ydotool → clipboard)
- `src/hotkey/` — Global hotkeys (hold-to-talk + toggle modes). `mod.rs` holds
  the chord parsing/matching both listeners share: `ll_hook.rs` (Windows),
  `linux_hotkey.rs` (evdev) + `gnome_grab.rs` (Mutter grab, so chords work over RDP)
- `src/config/` — TOML config (`mod.rs`), load migrations (`migrate.rs`),
  keyring API keys (`keys.rs`), vocabulary
- `src/logging.rs` — stdout + `beamer.log` in the config dir (previous run
  kept as `beamer.log.1`), panic hook, runtime level switch for the Debug toggle
- `src/notes/` — Notes + tasks over an automerge doc (`sync_doc/`, `doc_*`;
  `notes.json`/`tasks.json` are mirrors). Stores (`mod.rs`, `task_store.rs`),
  types (`model.rs`, `task.rs`), machine-local window state (`machine.rs`),
  stage writes (`lifecycle.rs`), size/delete edits (`edit.rs`), the extraction
  coroutine (`pipeline/`, `pipeline/sweep.rs`), live sync (`sync_client.rs`),
  legacy seed (`legacy.rs`), calendar export (`ics.rs`),
  the Done log (`accomplishments.rs`, `NoteKind::Accomplishment` notes)
- `src/llm/` — Client for the standalone llama.cpp server (Beamer never spawns
  it — except `src/components/`, which installs and starts the Windows ARM64
  server). Task extraction (`extract.rs`) over `chat.rs`; `prompts.rs` holds the
  extraction prompt; `client.rs` probes `/v1/models` on demand.
  ⚠️ **No crate-rooted paths in this directory** — `src/bin/task_eval.rs`
  `#[path]`-includes it, and there is no `src/lib.rs`.
- `src/components/` — Windows ARM64 only (empty catalog elsewhere). Everything
  the local extraction server needs (llama runtime, preset, launchers,
  Scheduled Task, model), declared in a catalog compiled into the exe and
  reconciled on launch. The only code allowed to touch the server's lifecycle.
  See `agent_docs/local_inference.md`.
  The UI's CSS, fonts and icon are compiled in too: never use manganis
  `asset!()` (`tests/packaging.rs` enforces it).
- `src/install/` — Non-Windows installers (the GNOME Shell helper extension)
- `src/update/` — Self-update from GitHub Releases (exe only — hence the rule above)
- `src/warmup.rs` — Cold-start warmup (keyring, audio, MPRIS, network) behind the splash
- `src/media.rs` — Pause-media-while-recording guard; `src/sounds.rs` — start/stop beeps
- `src/assets.rs` — Embedded icon + pill fonts; `src/tray/` — tray menu + icons
- `src/bin/` — `sync_server` (the sync relay), `task_eval` (extraction eval harness)
- `src/ui/` — Dioxus desktop. `app.rs` owns the signals and starts the workers
  (`app_setup.rs`, `app_menu.rs`, `app_pill.rs`, `app_splash.rs`); pages
  (`home.rs`, `history_page.rs`, `notes_page.rs`, `tasks_page/`, `vocab_page.rs`,
  `done_page.rs`, `settings/` — one file per card); sticky windows (`sticky*.rs`,
  `sticky_windows.rs`, `note_layout.rs`, `shell_window.rs`, `work_area.rs`);
  shared bits (`components.rs`, `fonts.rs`, `icons.rs`, `status_log.rs`)

## Commands

```
dx build                       # Dev build
dx build --release             # Release (use for injection testing)
dx serve                       # Run with hot reload
dx run                         # Run without hot reload
RUST_LOG=beamer=debug dx serve   # Run with debug logging (release logs: <config dir>/Beamer/beamer.log)
cargo test                     # Full suite (CI runs `cargo test --locked` with RUSTFLAGS="-D warnings", manual dispatch only)
cargo run --bin task_eval -- --limit 20   # Measure extraction against your own decisions
```

## Code Constraints

- Keep all files under 500 lines of code — split and refactor when approaching the limit
  (2 files currently exceed it: `src/main.rs`, `src/injection/clipboard.rs`
  — split on next touch, don't grow them further)
- Dioxus 0.7 owns the main thread and tokio runtime — never create a second runtime
- All UIA/Win32 calls MUST go through tokio::task::spawn_blocking() with COM initialized (sole exception: the low-level keyboard hook in `src/hotkey/ll_hook.rs`, which must own a real thread with a Windows message loop — `SetWindowsHookExW`/`GetMessageW` cannot run on the runtime's pool)
- tray-icon and muda used directly for the Menu/Icon values (not via Dioxus re-exports); the icon itself is registered through dioxus's `init_tray_icon`
- API keys stored in Windows Credential Manager via keyring — never on disk
- Design language: Beamer Purple (purple accent, 2px borders, hard-offset shadows, solid backgrounds)

## Git & Commits

- Do not add "Co-Authored-By: Claude" footers to commits — only use when actually writing code alongside the user, not for commit messages alone

## Detailed Docs (read before working on related code)

- `agent_docs/text_injection.md` — Per-platform fallback chains, per-app workarounds (CRITICAL)
- `agent_docs/transcription_backends.md` — API contracts, batch endpoints, retry/timeouts
- `agent_docs/audio_pipeline.md` — cpal setup, resampling, channels, buffer sizing (no VAD)
- `agent_docs/config_schema.md` — TOML schema, vocab format
- `agent_docs/dioxus_architecture.md` — Threading model, multi-window, tray integration
- `agent_docs/design_system.md` — Color tokens, typography, component patterns
- `agent_docs/sticky_notes.md` — Note windows, Wayland placement, cross-window state (CRITICAL for multi-window work)
- `agent_docs/local_inference.md` — The extraction pass, the pipeline coroutine, and the failures that return HTTP 200 (CRITICAL before touching `src/llm/`)
- `agent_docs/sync.md` — Cross-machine sync design (automerge doc, live sync client)
- `agent_docs/running_on_bearcave.md` — Running Beamer on the Windows laptop: local extraction server, install, first-run checklist
- `docs/decisions.md` — The "why" behind major shipped features, one paragraph each

---

## Current status

Version 1.0.5 on `master`. The exe is the whole install: UI assets and the
GNOME extension are compiled in, and `src/components/` brings the local
extraction server (runtime, preset, task, model) in line on launch —
Windows ARM64 only — so a self-update is all any machine needs. Sticky Notes
and cross-machine sync are shipped; note attachments (files, images, link
chips), the S1-mini cleanup pass, and both realtime transcription backends
have since been removed — extraction is now the only local model pass, and
every recording buffers mic PCM and POSTs it to one of 3 batch endpoints.
The GNOME extension is embedded in the
exe, so Settings → Injection can install it without a checkout; a GNOME
log-out/in is still needed to pick up a new extension version, but it's no
longer a separate manual deploy step. The
feature set is large and growing — run `cargo test` rather than trusting any
count here (CI runs the same suite with `-D warnings`, but only on manual
dispatch — there are no push/PR triggers).

The detailed docs are the source of truth:
- `agent_docs/sticky_notes.md` — Note windows, Wayland placement, cross-window state
- `agent_docs/local_inference.md` — The extraction pass, pipeline coroutine, and edge cases
- `agent_docs/sync.md` — Cross-machine sync design

`roadmap.md` tracks what's left: known issues, Linux edits awaiting a build,
and low-priority items. The 2026-09-26 stability review was triaged on
2026-09-30: fixed or dropped, nothing parked.

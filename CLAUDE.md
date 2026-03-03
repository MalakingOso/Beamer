# Beamer

Windows system-tray dictation app. Captures mic audio, transcribes via cloud APIs, injects text into the focused input field using a UI Automation fallback chain.

## Project Structure

- `src/main.rs` — Entry point, tray icon, Dioxus launch
- `src/audio/` — Mic capture (cpal) + voice activity detection (webrtc-vad)
- `src/transcription/` — Backend trait + 4 implementations (ElevenLabs/Mistral, batch/realtime)
- `src/injection/` — Text injection fallback chain (UIA → SendInput → clipboard)
- `src/hotkey/` — Global hotkey registration (hold-to-talk + toggle modes)
- `src/config/` — TOML config + vocabulary management
- `src/tray/` — System tray icon + menu
- `src/ui/` — Dioxus desktop: settings window, overlay, screen edge glow
  - `src/ui/settings/` — One file per card section (recording, transcription, api_keys, etc.)
  - `src/ui/components.rs` — Shared: Card, Select, Toggle, MaskedInput, TagChip

## Commands

```
cargo build                    # Dev build
cargo build --release          # Release (use for injection testing)
cargo run                      # Run
RUST_LOG=beamer=debug cargo run  # Run with debug logging
```

## Code Constraints

- Keep all files under 500 lines of code — split and refactor when approaching the limit
- Dioxus 0.7 owns the main thread and tokio runtime — never create a second runtime
- All UIA/Win32 calls MUST go through tokio::task::spawn_blocking() with COM initialized
- tray-icon and global-hotkey used directly (not via Dioxus re-exports)
- API keys stored in Windows Credential Manager via keyring — never on disk
- Design language: Deploy Purple (purple accent, 2px borders, hard-offset shadows, solid backgrounds)

## Git & Commits

- Do not add "Co-Authored-By: Claude" footers to commits — only use when actually writing code alongside the user, not for commit messages alone

## Detailed Docs (read before working on related code)

- `agent_docs/text_injection.md` — UIA fallback chain, per-app workarounds (CRITICAL)
- `agent_docs/transcription_backends.md` — API contracts, WebSocket protocols, retry
- `agent_docs/audio_pipeline.md` — cpal setup, VAD, sample rates, buffer sizing
- `agent_docs/config_schema.md` — TOML schema, vocab format
- `agent_docs/dioxus_architecture.md` — Threading model, multi-window, tray integration
- `agent_docs/design_system.md` — Color tokens, typography, Mica setup, component patterns

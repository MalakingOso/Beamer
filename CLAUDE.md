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

---

# 🚧 PICK UP HERE — Sticky Notes feature (paused 2026-08-20)

Design and planning are **done and committed**. No feature code written yet.
Read these two documents first, in order:

1. **Spec:** `docs/superpowers/specs/2026-08-20-sticky-notes-design.md`
2. **Plan:** `docs/superpowers/plans/2026-08-20-sticky-notes-phase1.md` (13 tasks)

**Next action:** execute **Task 1** of the Phase 1 plan — download the models and
benchmark them on the B60. Everything else waits on those numbers. Task 1 needs
the user present: it downloads ~5.6 GB and uses `sudo` to drop page cache for a
cold-start measurement.

## What the feature is

A second global hotkey dictates into a **sticky note** on the desktop instead of
injecting into the focused field. Two on-device model passes then run: a fast
cleanup pass rewrites the transcript in place, and a slower pass proposes action
items as **suggestions you accept or dismiss** — nothing enters a task list
unconfirmed. Delivered in three phases; only Phase 1 is planned in detail.

| Phase | Deliverable | Status |
|---|---|---|
| 1 | Dictate → sticky note on desktop, persisted, positioned | **Planned, ready to build** |
| 2 | S1-mini cleanup pass | Plan waits on Task 1 benchmark numbers |
| 3 | Task extraction, suggestion chips, Tasks page | Plan waits on an eval corpus from real Phase 1 use |

## ⬇️ Models to download from Hugging Face

Both are **Apache-2.0**, both are official **QAT / publisher** builds, and
neither `mmproj` (vision) file is needed — Beamer's use is text-only.

```bash
mkdir -p ~/models/beamer

# Stage 1 — transcript cleanup. 462 MB.
# Purpose-built ASR post-processor: punctuation, truecasing, filler removal,
# inverse text normalization. 94.8% token accuracy on 7,519 held-out cases,
# measured on THIS quant. Do not substitute f16.
hf download superwhisper/s1-mini-GGUF s1-mini-q4_k_m.gguf --local-dir ~/models/beamer
hf download superwhisper/s1-mini-GGUF LICENSE            --local-dir ~/models/beamer

# Stage 2 — task extraction. 5.15 GB. Ladder rung 1 (see spec §3).
hf download google/gemma-4-E4B-it-qat-q4_0-gguf gemma-4-E4B_q4_0-it.gguf \
  --local-dir ~/models/beamer
```

**Total resident VRAM: 5.61 GB of the B60's 22.7 GB**, leaving ~17 GB for other
GPU work on this machine.

**Model ladder** — the extraction model is chosen by *measurement against a
corpus of real notes*, not assertion. Promote only if precision is inadequate;
all are Google QAT + Apache-2.0, so promotion is a config change and a download:

| Rung | Model | Disk | Read/token |
|---|---|---|---|
| 1 (default) | `google/gemma-4-E4B-it-qat-q4_0-gguf` | 5.15 GB | ~5.2 GB |
| 2 | `unsloth/gemma-4-12B-it-qat-GGUF` (UD-Q4_K_XL) | 6.72 GB | ~6.7 GB |
| 3 | `google/gemma-4-26B-A4B-it-qat-q4_0-gguf` | 14.44 GB | **~2.2 GB (MoE)** |

Note rung 3 is both the **largest and the fastest** — 26B total but only ~4B
active, so it reads less per token than either dense option. If latency rather
than quality is the binding constraint, skip rung 2.

⚠️ **`superwhisper/s1-mini` is Apache 2.0 plus a binding naming term.** Any
product integrating it must identify it as `"S1-mini" by "Superwhisper"` — that
exact capitalization. This goes in the README and an in-app credits surface. It
is a licence condition, not a courtesy.

## ✅ Runtime decision: llama.cpp — rebuild it, don't switch to Ollama

**Use llama.cpp. Rebuild from master first** — the local checkout at
`/home/berkley/Programming/llama.cpp` is `e97492369`, **2026-04-13**, four months
stale. Keep the existing **Vulkan** configuration (`GGML_VULKAN=ON`,
`GGML_SYCL=OFF`); Vulkan needs no oneAPI environment sourced, which matters for a
process Beamer spawns from a desktop session.

**Ollama was evaluated and rejected.** Its standard release does not support
Intel Arc; Intel's supported path is the IPEX-LLM fork, which is retired software
past end-of-life and off-limits on this machine. The remaining option is an
unofficial community Vulkan build — strictly worse than the working Vulkan
llama.cpp already on disk. Ollama's one real advantage was model lifecycle
management, and that is now moot (below).

**Use the llama.cpp router server — one process, both models.** The current build
already has it (`tools/server/server-models.cpp`), which deleted a whole
subsystem from the original design:

```bash
llama-server --models-dir ~/models/beamer --models-max 2 \
             --host 127.0.0.1 --port 8080 -ngl 99 -c 8192 --jinja
```

- Models are addressed by name in the ordinary OpenAI request: `"model": "s1-mini-q4_k_m"`
- `GET /models` — health and load state (replaces hand-rolled `/health` polling)
- `POST /models/load` — warm S1-mini at startup
- `POST /models/unload` — release the 5 GB extraction model when idle
- Set `LD_LIBRARY_PATH` to the llama.cpp `build/bin` or it fails on `libmtmd.so.0`
- Set `GGML_VK_VISIBLE_DEVICES` to the **B60's Vulkan index** — read it from
  `llama-server --list-devices`, do not assume it is 0

**GPU: the B60 (`0000:0a:00.0`), not the B570.** Verified by display topology, not
preference — `card1-DP-1` and `card1-DP-4` are connected, so the **B570 drives
both monitors**; the B60 has zero connected outputs and is pure headless compute.

## 🪟 Wayland window positioning — the thing that looks broken but isn't

Sticky notes need to reappear where you left them. On Wayland that is genuinely
impossible for an ordinary client, and the API that appears to work lies:

- **No protocol exists.** Core Wayland and `xdg-shell` give clients no way to
  learn or set absolute position. The compositor owns placement, by design.
- **`tao::Window::outer_position()` returns `Ok((0,0))`, not an error.** It reads
  a cached atomic fed by GDK `frame_extents()` on `configure_event`
  (`tao-0.34.8/src/platform_impl/linux/window.rs:465`), and GDK has no global
  coordinates under Wayland. **Never call it on Wayland** — it silently persists
  garbage.
- **`xx-session-management-v1`** is the official fix, and its design confirms the
  rule: the app never learns coordinates; it names a window and asks the
  compositor to restore it. Qt 6.10 and SDL (Mar 2026) implement it. No
  `xx_session_management_v1` symbol was found in `libmutter-18.so.0` or
  `/usr/bin/gnome-shell` on this box (GNOME 50.1), so it is not usable yet.
- **`vixalien/sticky`, the app this was modelled on, does not persist positions
  at all** — only width/height/open. That is the state of the art.

**Beamer's answer: our own GNOME Shell extension.** Code inside GNOME Shell is not
a Wayland client and is not subject to any of the above. Extension v5 gains two
D-Bus methods on the existing `app.beamer.FocusProvider`:

- `PlaceWindow(title, x, y, all_workspaces)` → `Meta.Window.move_frame()` + `stick()`
- `GetWindowFrame(title)` → `Meta.Window.get_frame_rect()`

Windows are matched by exact title, `Beamer Note <id>`. Most apps can't justify
shipping an extension for this; we already ship one for text injection and the
recording pill, so it costs two methods.

⚠️ **GNOME extensions do not hot-reload on Wayland.** Every extension change
needs a full log out and back in. Budget for it; skipping it means testing stale
code and drawing false conclusions.

When Mutter exposes `xx-session-management-v1` unflagged, that becomes the
portable path and this dependency can be dropped.

## Decisions already made — don't relitigate without new information

- Notes are **plain Dioxus windows**, deliberately **not always-on-top**.
- Two models, not one. S1-mini is not a chat model and cannot do extraction;
  a general model prompted to punctuate is worse and slower at cleanup.
- Extraction is a **precision** problem, not an extraction problem. Strict
  policy: first-person commitments only. Aspirations are excluded on purpose.
- Tasks arrive as **suggestions**, never auto-added. Dismissed rows are
  **retained**, not deleted — they are the labelled negatives for the eval corpus.
- Meeting capture / system audio / diarization: **explicitly out of scope.**
- **Every latency number in the spec is an estimate.** Task 1 exists to replace
  them with measurements, and may invalidate the Phase 2 design.

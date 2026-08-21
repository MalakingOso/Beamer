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

# 🚧 PICK UP HERE — Sticky Notes feature (Tasks 1–3, 5–8 done 2026-08-21)

Read these three documents first, in order:

1. **Spec:** `docs/superpowers/specs/2026-08-20-sticky-notes-design.md`
2. **Plan:** `docs/superpowers/plans/2026-08-20-sticky-notes-phase1.md` (13 tasks)
3. **Benchmarks:** `docs/superpowers/benchmarks/2026-08-20-b60-llama-vulkan.md`

**Tasks 1, 2, 3, 5, 6, 7 and 8 are complete** on branch `feat/sticky-notes`.
Task 1 replaced every latency estimate with a measurement (two decisions changed
as a result, below). Batches A–C then built the feature through to its first
visible payoff: **press the note hotkey, speak, and a sticky note appears on the
desktop and survives a restart.** 121 tests pass, up from a 101 baseline.

**Task 4 (Windows hotkey parity) is deferred** — `ll_hook.rs` is
`#[cfg(target_os = "windows")]` and cannot be built or tested here.

**Next action:** execute **Task 9** — the GNOME extension's `PlaceWindow` /
`GetWindowFrame` D-Bus methods. ⚠️ It needs a **full GNOME log out and back in**
mid-task; extensions do not hot-reload on Wayland. Until Tasks 9/10 land, notes
reappear wherever Mutter puts them, which is expected, not a bug.

**Set a note hotkey before testing** — `note_hotkey = ""` in `config.toml` means
note capture is off entirely, by design, so the dictation hotkey can never be
silently diverted.

Six corrections to the repo plan were found while executing Batches A–C and are
written up in a `## Corrections found while executing Batches A–C` section in the
plan file. The one that matters most: **nothing in Tasks 2–8 ever wrote
`notes.json`** — the flush driver only appeared in Task 11 — so notes were created
and never persisted. Fixed three ways (immediate flush on capture, a 500 ms
debounce tick, and a flush before the tray Quit's `process::exit`).

**Measured, so stop estimating:**

| Thing | Measured |
|---|---|
| S1-mini cleans a ~60-word note | **0.225 s** (threshold was 1.5 s) |
| Gemma extraction (thinking off) | **1.11 s** |
| Wake extraction model from sleep | **1.68 s** (threshold was 10 s) |
| Cold start (spawn + 4.8 GB load) | 4.06 s |
| Resident VRAM, both models | ~4.3 GB of 22.7 GB |

Phase 2 proceeds as specced. Asymmetric idle shutdown is confirmed correct.

## What the feature is

A second global hotkey dictates into a **sticky note** on the desktop instead of
injecting into the focused field. Two on-device model passes then run: a fast
cleanup pass rewrites the transcript in place, and a slower pass proposes action
items as **suggestions you accept or dismiss** — nothing enters a task list
unconfirmed. Delivered in three phases; only Phase 1 is planned in detail.

| Phase | Deliverable | Status |
|---|---|---|
| 1 | Dictate → sticky note on desktop, persisted, positioned | **Tasks 1–3, 5–8 done; Task 9 (positioning) next** |
| 2 | S1-mini cleanup pass | **Unblocked** — numbers measured, 0.225 s |
| 3 | Task extraction, suggestion chips, Tasks page | Plan waits on an eval corpus from real Phase 1 use |

## ⬇️ Models — downloaded, in `~/models/beamer/`

Both are official **QAT / publisher** builds. Neither `mmproj` (vision) file is
needed — Beamer's use is text-only.

| File | Size | Role |
|---|---|---|
| `s1-mini-q4_k_m.gguf` | 462 MiB | Stage 1 cleanup. 94.8% token accuracy on 7,519 held-out cases, measured on **this** quant — do not substitute f16. |
| `gemma-4-E4B_q4_0-it.gguf` | 4.80 GiB | Stage 2 extraction. Ladder rung 1. |

Licences are in `licenses/`; required attribution is in `README.md`.

**Measured resident VRAM: ~4.3 GB of the B60's 22.7 GB** — better than the
5.61 GB the spec estimated. Extraction model alone is ~3.1-3.4 GB.

**Model ladder** — the extraction model is chosen by *measurement against a
corpus of real notes*, not assertion. Promote only if precision is inadequate;
all are Google QAT + Apache-2.0, so promotion is a config change and a download:

| Rung | Model | Disk | Note |
|---|---|---|---|
| 0 | `google/gemma-4-E2B-it-qat-q4_0-gguf` | 3.12 GiB | Smaller/faster than the default; downgrade option if latency binds |
| 1 (default) | `google/gemma-4-E4B-it-qat-q4_0-gguf` | 4.80 GiB | |
| 2 | `unsloth/gemma-4-12B-it-qat-GGUF` (UD-Q4_K_XL) | 6.72 GB | |
| 3 | `google/gemma-4-26B-A4B-it-qat-q4_0-gguf` | 13.45 GiB | 26B total, only ~4B active — reads far less per token than either dense option |

Rung 3 is both the **largest on disk and potentially the fastest per token**.
Its cost is load time, which residency makes irrelevant: with ~18 GB of the B60
idle, `sleep-idle-seconds = -1` keeps it resident and load time stops mattering.

⚠️ **`superwhisper/s1-mini` is Apache 2.0 plus a binding naming term.** Any
product integrating it must identify it as `"S1-mini" by "Superwhisper"` — that
exact capitalization. This goes in the README and an in-app credits surface. It
is a licence condition, not a courtesy.

## ✅ Runtime: llama.cpp, **SYCL**, standalone server — Beamer is a client

**Two decisions changed on 2026-08-21. Do not revert them from the older text in
the spec; the spec has been updated to match.**

**1. Backend is SYCL, not Vulkan.** The rebuild is done (`e97492369` ->
`5a32f7b66`). Two build dirs exist:

- `build-sycl/` — **use this.** `GGML_SYCL=ON`, `GGML_SYCL_F16=ON`, icx/icpx.
- `build/` — Vulkan fallback, no oneAPI runtime needed.
- `build.stale-pre-20260821/` — the April binaries, kept for rollback.

Measured, same commit, same device: SYCL is **2.35x** Vulkan at prompt
processing, **~1.2x** at generation. The original spec chose Vulkan because it
needs no oneAPI environment — valid only while Beamer spawned the server, which
it no longer does.

**2. The server is standalone. Beamer never spawns it.** Deployment lives in
`deploy/`:

```bash
# start it
systemctl --user enable --now llama-beamer        # deploy/llama-beamer.service
# or ad hoc — note LD_LIBRARY_PATH must EXTEND oneAPI's, not replace it
source ~/intel/oneapi/2025.3/oneapi-vars.sh
export LD_LIBRARY_PATH=~/Programming/llama.cpp/build-sycl/bin:$LD_LIBRARY_PATH
llama-server --models-dir ~/models/beamer \
  --models-preset ~/Programming/Beamer/deploy/llama-models.ini \
  --models-max 2 --host 127.0.0.1 --port 8080
```

Beamer's whole config for this is `base_url` — see spec §10.

### Four traps, each of which cost real time to find

⚠️ **Both models produce garbage unless explicitly told not to reason.** Both
failures look like a healthy server returning a valid response.

- **S1-mini** inherits Qwen3's template, which defaults thinking ON; it was
  trained OFF. Without `chat-template-kwargs {"enable_thinking":false}` it emits
  `<think>` and stops after 3 tokens. Do **not** substitute `reasoning-budget 0`
  — the model card says output degrades. Also force greedy decoding: the GGUF
  carries `temp 0.6 / top_p 0.95 / top_k 20` inherited from Qwen3-0.6B.
- **Gemma 4** reasons by default, filling `reasoning_content` while `content`
  stays empty. Thinking on cost 4x the latency for identical extraction output.

Both are already set in `deploy/llama-models.ini`.

⚠️ **Never poll the server on a timer.** Status reads reset the per-model idle
clock. A background health check pins the ~3 GB extraction model in VRAM
forever, with no error and no symptom. Probe on demand only.

⚠️ **`-dev SYCL1`, not an env mask.** `ZE_AFFINITY_MASK` / `GGML_VK_VISIBLE_DEVICES`
*filter* the device list, so the B60 re-indexes to 0 and the mask value stops
matching the in-process id. `-dev` selects from the full list. The B60 is
index **1** on both backends — verified, not assumed.

⚠️ **`LD_LIBRARY_PATH` must extend oneAPI's, not replace it.** Setting it
outright drops `libsvml.so` and the server dies naming a library nothing else
mentions.

### Rejected on measurement — do not re-propose without new evidence

- **n-gram speculative decoding** (`--spec-default`). Looked like a 5x win;
  that was an artifact of re-running identical text and reading cached output.
  On unseen notes it does not engage at all, and tuned to shorter matches it
  accepts ~30% and nets a wash (254-313 t/s vs a ~280 baseline). The cleanup
  rewrite breaks n-gram matches — near-copy is not exact-copy.
- **Ollama.** Standard release has no Intel Arc support; Intel's path is the
  retired IPEX-LLM fork, off-limits on this machine. Its one advantage was model
  lifecycle management, which the llama.cpp router now does natively.

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

# Beamer

Windows system-tray dictation app. Captures mic audio, transcribes via cloud APIs, injects text into the focused input field using a UI Automation fallback chain.

## Project Structure

- `src/main.rs` — Entry point, tray icon, Dioxus launch
- `src/audio/` — Mic capture (cpal) + voice activity detection (webrtc-vad)
- `src/transcription/` — Backend trait + 4 implementations (ElevenLabs/Mistral, batch/realtime)
- `src/injection/` — Text injection fallback chain (UIA → SendInput → clipboard)
- `src/hotkey/` — Global hotkey registration (hold-to-talk + toggle modes)
- `src/config/` — TOML config + vocabulary management
- `src/notes/` — Note store (`mod.rs`) + types (`model.rs`), stage transitions
  (`lifecycle.rs`), attachment/size/delete edits (`edit.rs`), the model-pass
  coroutine (`pipeline.rs`), task suggestions (`task.rs`, `task_store.rs`),
  calendar export (`ics.rs`)
  - `blocks.rs` — the `[[beamer:<id>]]` placeholder grammar. Pure. **Read it
    before touching the body of a note or the cleanup path** — tokens must
    never reach a model.
- `src/llm/` — Client for the standalone llama.cpp server (Beamer never spawns
  it). Cleanup (`cleanup.rs`) and extraction (`extract.rs`) over `chat.rs`;
  `prompts.rs` holds both models' input contracts.
  ⚠️ **No crate-rooted paths in this directory** — `src/bin/task_eval.rs`
  `#[path]`-includes it, and there is no `src/lib.rs`.
- `src/tray/` — System tray icon + menu
- `src/ui/` — Dioxus desktop: settings window, overlay, screen edge glow
  - `src/ui/settings/` — One file per card section (recording, transcription, api_keys, etc.)
  - `src/ui/sticky*.rs`, `note_layout.rs`, `shell_window.rs` — Sticky note
    windows, placement; `sticky_blocks.rs` renders the block stack and serves
    a note's images to its own webview
  - `src/ui/notes_page.rs` — All-notes board; `tasks_page.rs` — accepted tasks
  - `src/ui/components.rs` — Shared: Card, Select, Toggle, MaskedInput, TagChip

## Commands

```
cargo build                    # Dev build
cargo build --release          # Release (use for injection testing)
cargo run                      # Run
RUST_LOG=beamer=debug cargo run  # Run with debug logging
cargo run --bin task_eval -- --limit 20   # Measure extraction against your own decisions
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
- `agent_docs/sticky_notes.md` — Note windows, Wayland placement, cross-window state (CRITICAL for multi-window work)
- `agent_docs/local_inference.md` — The two model passes, the pipeline coroutine, and the failures that return HTTP 200 (CRITICAL before touching `src/llm/`)

---
# 🚧 PICK UP HERE — Sticky Notes Phase 1 (complete except one log out)

**Read `agent_docs/sticky_notes.md` first.** It carries everything below in
durable form, plus the facts that cost real time to learn.

Phase 1 is **built and committed** on `feat/sticky-notes`. **168 tests pass**,
up from a 101 baseline (121 at the end of Batches A–C). Zero build warnings.

## ✅ The log out has happened — extension v5 is live

Confirmed 2026-08-21: `GetVersion` returns `(uint32 5,)` and introspection
lists both `PlaceWindow` and `GetWindowFrame`. The stale-v3 caveat that used to
sit here is resolved — notes are placed, and the note pill shows its own state
rather than "Transcribing…" with an idle sweep.

✅ **`PlaceWindow` verified to actually move a window** (2026-08-21): placed a
live note at 400,300 and `GetWindowFrame` read back exactly
`(true, 400, 300, 320, 260)`. At scale 1.0 physical and logical pixels agree,
so this is confirmation rather than proof against a scaling bug.

⚠️ Re-check the version after any future extension edit — GNOME extensions do
not hot-reload on Wayland, so every change still needs a full log out.
`gnome-extensions list --details` reports the shell's *cached* version and
cannot be used for this; only the `GetVersion` call above is authoritative.

```bash
gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.GetVersion      # expect (uint32 5,)
```

Then, with a note open (substitute a real id from `~/.config/Beamer/notes.json`):

```bash
gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.PlaceWindow \
  "Beamer Note <real-id>" 400 300 true              # expect (true,) AND visible movement

gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.GetWindowFrame \
  "Beamer Note <real-id>"                           # expect ~(true, 400, 300, w, h)
```

`(true,)` with no movement means `move_frame` is being ignored — stop and
investigate before trusting anything downstream. The `GetWindowFrame` read-back
is what would catch a physical-vs-logical pixel error; note that at scale 1.0
the two agree, so a clean read is confirmation, not proof.

### End-to-end checklist

⚠️ **Turn on "Note capture" in Settings → Recording first.** Empty
`note_hotkey` means note capture is off entirely, by design, so the dictation
hotkey can never be silently diverted. The switch proposes **Ctrl+Alt+Space**;
the picker below it edits the chord, and both re-register live (no restart).

⚠️ **Do not pick `Ctrl+Super+<key>` for notes.** Two reasons, both verified:
`HotkeyConfig` has no Meta modifier field, so Super is silently dropped; and
even with that fixed, dictation's `Ctrl+Super` is a strict *prefix* — pressing
Ctrl then Super fires `RecordStart(Inject)` before the key is reached.

1. Dictation hotkey → text injects as before. *(The regression that matters most.)*
2. Note hotkey → sticky appears, pill shows the **purple note ring**, and the
   waveform still tracks your mic.
3. Dictate four or five notes → **spread irregularly**, none stacked, none off-screen.
4. Close one with Alt+F4 → `notes.json` shows `"open": false`.
5. Notes board (new sidebar icon): search finds a note by a word you actually
   *said*; clicking a card reopens it; archive hides it; "Show archived" restores.
6. Restart with several notes open → they reappear, freshly scattered. Positions
   deliberately do **not** match the previous session.
7. Local AI card with the server up → lists both models and their states. Then
   `pkill -x llama-server` (**never** `pkill -f`, which matches the shell running
   it) → card reports not running, notes still captured.

## Scope change: notes are placed, not remembered

**Position persistence was dropped by decision (2026-08-21).** `place_next`
scatters each note around the ones already on screen, freshly, every launch.
Restart and they reappear *elsewhere* — correct, not a bug. This removed the
least reliable part of the design (reading a window's geometry back on Wayland)
and replaced it with a pure, tested function. Recorded in the spec §8 and
`agent_docs/sticky_notes.md` so nobody "restores" it as a missing feature.

## What is NOT done

- **Task 4 — the Windows target does not compile.** `src/hotkey/ll_hook.rs:137,143`
  still construct `HotkeyEvent::RecordStart` with no payload. It is
  `#[cfg(target_os = "windows")]`, so Linux builds stay green. This is a **build
  fix**, not parity work. See `todo.md`.
- Note **size** is never captured, so resizing is forgotten. Unlike position,
  this has no design justification.
- Phases 2 and 3 are **built** — see `agent_docs/local_inference.md`. Phase 3's
  prompt is untuned: it was spot-checked against the live model, not measured
  against a corpus, because none existed. Precision rests on accept/dismiss and
  evidence grounding, not on the prompt being right first time.

**Measured, so stop estimating:**

| Thing | Measured |
|---|---|
| S1-mini cleans a ~60-word note | **0.225 s** (threshold was 1.5 s) |
| Gemma extraction (thinking off) | **1.11 s** |
| Wake extraction model from sleep | **1.68 s** (threshold was 10 s) |
| Cold start (spawn + 4.8 GB load) | 4.06 s |
| Resident VRAM, both models | ~4.3 GB of 22.7 GB |

## What the feature is

A second global hotkey dictates into a **sticky note** on the desktop instead of
injecting into the focused field. Two on-device model passes then run: a fast
cleanup pass rewrites the transcript in place, and a slower pass proposes action
items as **suggestions you accept or dismiss** — nothing enters a task list
unconfirmed. Delivered in three phases; only Phase 1 is planned in detail.

| Phase | Deliverable | Status |
|---|---|---|
| 1 | Dictate → sticky note on desktop, persisted, positioned | **Done** |
| 2 | S1-mini cleanup pass | **Done** — dictate and the note tidies itself |
| 3 | Task extraction, suggestion chips, Tasks page | **Built, untuned.** No eval corpus existed, so the prompt landed on a spot-check. `cargo run --bin task_eval` measures it against your own accept/dismiss decisions as they accumulate. |

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
| 3 | `google/gemma-4-26B-A4B-it-qat-q4_0-gguf` | 13.45 GiB | **The slowest measured**, 43.6 t/s — see below |

⚠️ **Corrected on measurement.** An earlier version of this file claimed rung 3
was "potentially the fastest per token", reasoning from bytes read per token.
Measured on the B60 it is the **slowest**: 43.6 t/s against E4B's 77.3 and
E2B's 116.0. The read-per-token argument assumes bandwidth-bound decoding, and
expert routing and gather overhead dominate here. On this hardware the ladder
is monotonic in size — **smaller is faster** — so rung 3 is a *quality* option
only, never a speed play.

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

Both are now set in `deploy/llama-models.ini`. ⚠️ **This file previously
claimed they both already were, and only s1-mini's was.** Gemma had been
reasoning on every request since the preset was written. Nothing showed it —
200, valid JSON, byte-identical extracted tasks — but it cost 306 predicted
tokens / 4069 ms against 64 tokens / 842 ms with thinking off. Measured
2026-08-22. If you add a model to the preset, its thinking switch is not
optional, and no test can catch its absence for you: Beamer deliberately sends
no template parameters of its own, so the preset is the only owner.

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

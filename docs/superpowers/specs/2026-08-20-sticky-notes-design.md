# Sticky Notes with On-Device AI — Design

**Date:** 2026-08-20
**Status:** Draft for review
**Target platform:** GNOME Wayland / Mutter (primary). Windows kept compiling, not optimized for.

## 1. Summary

A second global hotkey dictates into a **sticky note** instead of injecting text
into the focused field. The note window appears immediately with the raw
transcript. Two on-device model passes then run in the background: a fast
cleanup pass rewrites the transcript into properly punctuated prose in place,
and a slower analysis pass extracts action items into a global **Tasks** page
inside Beamer.

No cloud service is involved in the AI passes. Speech-to-text continues to use
the existing cloud backends, because that path is already fast and already
built.

### Goals

- Dictate a thought and have it land on the desktop as a note, without
  interrupting whatever is focused.
- The note reads like written English, not like a transcript.
- Action items buried in the note surface as checkable tasks.
- All text analysis happens locally on this machine's GPUs.
- Notes persist across restarts, including where they sit on screen.

### Non-goals

- Meeting capture, system/loopback audio, or diarization. Explicitly dropped.
- Note sharing, sync, or export to third-party task managers.
- Replacing cloud speech-to-text with a local ASR model. Possible later; out of
  scope here.
- Rich text. Notes are plain text plus a color.

## 2. Verified environment facts

Everything in this section was checked on this machine on 2026-08-20 rather
than assumed. Numbers marked *(estimate)* were not benchmarked and must be
measured during implementation.

| Fact | Value | How verified |
|---|---|---|
| GPU 1 | Intel Arc B570 (BMG G21), 10 GB, `0000:03:00.0`, `card1` | `/sys/class/drm`, `vulkaninfo` |
| GPU 2 | Intel Arc Pro B60 (BMG G21), 22.7 GB, `0000:0a:00.0`, `card2` | same |
| **Display topology** | **B570 drives both monitors** (`card1-DP-1`, `card1-DP-4` connected). **B60 has zero connected outputs** — headless compute. | `/sys/class/drm/*/status` |
| Vulkan driver | Mesa 26.1.7 (kisak-mesa PPA), `DRIVER_ID_INTEL_OPEN_SOURCE_MESA`, both Arc cards enumerated | `vulkaninfo --summary` |
| llama.cpp | built at `/home/berkley/Programming/llama.cpp/build/bin/`, version 8782 (`e97492369`) | `llama-server --version` |
| llama.cpp backend | **Vulkan** (`GGML_VULKAN:BOOL=ON`, `GGML_SYCL:BOOL=OFF`, `GGML_CUDA:BOOL=OFF`) | `build/CMakeCache.txt` |
| llama.cpp arch support | includes `gemma4`, `qwen3`, `qwen35`, `qwen35moe`, `lfm2`, `lfm2moe`, `nemotron_h_moe`, `minimax-m2`, `kimi-linear`. **No DeepSeek-V4 arch** — that family cannot run on this build. | `src/llama-arch.cpp` |
| **MTP unsupported** | `// NextN/MTP tensors are currently ignored (reserved for future MTP support)`. Published `mtp-*.gguf` draft weights are unusable here. Classic `--model-draft` speculative decoding **is** supported. | `src/llama-arch.cpp:757`, `common/arg.cpp` |
| Running inference servers | none (nothing on 8000/8001/11434, no ollama installed) | `ss -ltnp`, `which` |
| Disk free | 649 GB | `df -h` |
| GNOME extension | `beamer-focus@beamer.app` v4, exports `app.beamer.FocusProvider` at `/app/beamer/FocusProvider` | `extension/…/extension.js:20` |
| Existing multi-window pattern | `window.new_window(dom, cfg).await` | `src/ui/app_setup.rs:102`, `:194` |

Note the llama-server binary needs `LD_LIBRARY_PATH=<build>/bin` to resolve
`libmtmd.so.0`; it fails to start without it. Beamer must set this when
spawning.

### Why Vulkan rather than SYCL or vLLM

The existing build is Vulkan and it works. Vulkan on Arc goes through Mesa,
which means **no oneAPI environment needs to be sourced** — decisive for a
process Beamer spawns unattended from a desktop session that has not run
`oneapi-vars.sh`.

vLLM (available at `/home/berkley/Programming/vllm`, validated with torch
`2.12.1+xpu`) was considered and rejected for this use: it is built for
high-throughput batch serving, takes tens of seconds to start, and holds GPU
memory aggressively. This workload is a handful of very short requests per
day. `llama-server` starts in about a second with a small model and can be
idled out.

In-process inference via a Rust crate (candle, mistral.rs) was rejected: no
mature Intel-GPU backend, and Dioxus 0.7 already owns the process's only tokio
runtime (`agent_docs/dioxus_architecture.md`), so hosting a second heavyweight
compute engine in-process invites exactly the threading problems that document
warns about.

## 3. Model selection

Two models, two jobs. Splitting them is what makes the fast path fast, and it
is forced anyway: the stage-1 model is not a chat model and cannot do stage 2.

Both run on the **B60**. That is not a preference — the B570 drives both
monitors and is busy compositing GNOME, while the B60 has no connected outputs
at all. Its full 22.7 GB is available with no framebuffer and no compositor
competing for bandwidth.

### Stage 1 — cleanup: `superwhisper/s1-mini-GGUF`, Q4_K_M

The single best-fitting model found. It is not a general model prompted to
punctuate; it is a text normalizer trained for exactly this transformation.

| Property | Value |
|---|---|
| File | `s1-mini-q4_k_m.gguf`, **462 MB** |
| Parameters | 0.6B unique (751.6M tensor elements; embeddings are tied but materialized twice) |
| Base | Qwen3-0.6B finetune; GGUF arch `qwen3`, 311 tensors, 28 blocks |
| Context | 40,960 tokens |
| Measured accuracy | **94.8% token accuracy** on 7,519 held-out English cases — *measured on this Q4_K_M build* |
| License | Apache 2.0 **plus an additional naming term** (see §3.3) |

**Q4_K_M, not F16.** The 1.4 GB F16 build exists, but Q4_K_M is the build the
publisher recommends and the one the 94.8% figure was measured on. Q4_K_M keeps
the 29 most quantization-sensitive tensors at Q6_K and normalization params at
F32. Choosing F16 would trade 1 GB of VRAM for an unmeasured quality delta.

What it does: removes fillers, resolves false starts and self-corrections to
the value the speaker landed on, applies punctuation and capitalization, and
renders spoken numbers, dates, times, currency and email addresses in written
form.

**It is English-only and not a chat model.** It will not follow general
instructions. This is a feature — it cannot wander off and "improve" your
note — but it means stage 2 must be a separate model.

#### Mandatory input format

The model was trained on an exact input shape. The publisher's documentation
states plainly that deviating from it causes hallucination or garbled output.
This is not a prompt to tune; it is a wire format.

System prompt, verbatim:

```
You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text.
```

User message — control line, newline, then the transcript:

```
[Styling: semi-formal] [Structure: prose] [Context: general]
<raw transcript>
```

| Axis | Trained values | Beamer's default |
|---|---|---|
| `Styling` | `casual`, `semi-casual`, `semi-formal`, `formal` | `semi-formal` (the publisher's suggested default: standard written English, contractions kept, colloquialisms smoothed) |
| `Structure` | `prose`, `lists` | `lists` — a dictated note that enumerates things should become bullets; the model is conservative and needs 3+ items before it will |
| `Context` | `general`, `email` | `general` |

Styling is exposed as a per-note setting in the UI, defaulting from config.
Values outside the trained sets must never be sent.

#### Empty output is a valid result

Filler-only or noise-only input correctly returns an **empty string**. This is
success, not failure. Beamer keeps `raw` as `body` in that case and marks the
note `Cleaned`, not `CleanFailed` — blanking a note because the speaker only
said "um" would destroy the record of a bad recording.

### Stage 2 — task extraction: `google/gemma-4-26B-A4B-it-qat-q4_0-gguf`

| Property | Value |
|---|---|
| File | `gemma-4-26B_q4_0-it.gguf`, **14.44 GB** |
| Architecture | Gemma 4 MoE — 26B total, **~4B active**; 128 experts, top-8 routed |
| Layers | 30; `sliding_window: 1024`, full attention only every 6th layer; `attention_k_eq_v: true` |
| Context | 262,144 max (Beamer uses 8,192) |
| Quantization | **QAT** — quantization-aware trained, published by Google itself |
| License | Apache-2.0 per the Hub tag (verify at download — some third-party derivatives are tagged `license:gemma`) |

Two properties make this the pick:

**MoE is the decisive win on a bandwidth-bound GPU.** Decode speed on Arc via
Vulkan is limited by how many bytes must be read per token. A dense 27B at Q4
reads its whole ~16 GB every token. This model reads only its 8 active experts
of 128 — roughly 2 GB. Same VRAM footprint on disk, roughly 3–4× the decode
speed *(estimate; see Risks)*.

**QAT is not an ordinary quant.** The model was trained with q4_0 quantization
in the loop, so quality at 4 bits tracks the bf16 model rather than degrading
from it. Every alternative below ships post-training imatrix quants instead.

The `mmproj` file (vision projector, 1.19 GB) is **not** downloaded — Beamer's
use is text-only.

Sliding-window attention on 25 of 30 layers plus shared K/V keeps the KV cache
unusually small for a 26B model, which is why 8K context costs little.

### VRAM budget

| Item | Size |
|---|---|
| `gemma-4-26B_q4_0-it.gguf` | 14.44 GB |
| `s1-mini-q4_k_m.gguf` | 0.46 GB |
| **Weights total** | **14.90 GB** |
| Available on B60 | 22.7 GB |
| **Headroom for KV caches, buffers, fragmentation** | **~7.8 GB** |

Both models stay resident simultaneously. No swapping, no reload stalls.

### Alternatives considered and rejected

| Candidate | Size | Why not |
|---|---|---|
| `NVIDIA-Nemotron-3.5-Lightning-30B-A3B` | ~17 GB Q4 | Closest rival — 3B active, GGUF published by **ggml-org itself**. Rejected on three counts: `license: other` vs Apache-2.0; post-training imatrix quants vs QAT; and it is a **mamba2 hybrid**, and SSM kernels are the least-mature path in llama.cpp's Vulkan backend. A performance surprise on Intel is likely enough to matter. **Documented as the primary A/B alternative.** |
| `ornith-ai/Ornith-1.5-35B-A3B-GGUF` | **21.7 GB** Q4_K_M | Does not fit alongside stage 1 with usable KV cache. Also 2 days old at time of writing, from an unproven org. |
| `Qwen3.8-27B` GGUF | ~16 GB Q4 | Dense. Same VRAM, ~3–4× slower decode than a comparable MoE. |
| `google/gemma-4-31B-it` | ~18 GB Q4 | Dense, larger, slower, no QAT build. |
| `empero-ai/Qwen3.8-9B-Distill-GGUF` | ~9.5 GB Q8 | Dense 9B — fits easily but is a quality step down from a 26B MoE for no speed gain worth having. Reasonable fallback if the 26B disappoints on latency. |
| `LiquidAI/LFM2.5-2.6B-GGUF` | ~2 GB | The previous draft's pick, sized for a 10 GB card. Overtaken now that the budget is 22.7 GB. |
| `unsloth/…-qat-GGUF` UD-Q4_K_XL | 14.25 GB | Unsloth's dynamic K-quant of the same QAT weights. Arguably better bit allocation than legacy q4_0, but q4_0 is the scheme QAT actually targeted. Worth A/B testing; not the default. |
| `Cactus-Compute/needle2` | — | On-device tool-calling specialist, but `cactus-needle` format — a whole second runtime. |
| DeepSeek-V4 family | — | **No `deepseek_v4` arch in this llama.cpp build.** Cannot run. |
| Any `mtp-*.gguf` draft weights | — | MTP tensors are explicitly ignored by this build (§2). |

### 3.3 Attribution obligation

`s1-mini`'s license is Apache 2.0 **with an additional term**, quoted in full:

> In addition to the terms of the Apache License, Version 2.0 above: any use,
> distribution, or integration of this model, whether unmodified or as part of
> a derivative work or product, must continue to identify it by its original
> name, "S1-mini" by "Superwhisper", using that exact capitalization,
> regardless of any other name under which the model or a product
> incorporating it is marketed or distributed.

This is binding and easy to satisfy, but it is **not optional**. Beamer must
carry the string `"S1-mini" by "Superwhisper"` — exact capitalization — in an
About/credits surface in the settings UI and in the repository's README. This
is a required implementation task, not a nicety.

Apache 2.0 also requires shipping a copy of the license and its attribution
notices. Both model licenses go in a `licenses/` directory in the repo.

## 4. Architecture

```
note-mode hotkey
      │
      ▼
orchestrator (existing audio → ASR path, unchanged)
      │  Sink::Note
      ▼
notes::store ──── creates Note { state: Raw } ──► sticky window opens immediately
      │
      ▼
llm::cleanup  (S1-mini)   ──► Note.body updated in place, state: Cleaned
      │
      ▼
llm::extract  (gemma-4-26B-A4B) ──► Task rows created, state: Analyzed
      │
      ▼
Tasks page badge
```

The two LLM stages run as one background tokio task, **sequentially**:
extraction reads `body` after cleanup has improved it, because punctuated prose
is materially easier to extract action items from than a raw transcript.

Neither stage blocks note creation — the sticky window is already on screen
before stage 1 starts. If cleanup fails, extraction still runs, against `raw`.
Each stage is independently retryable from the note's UI.

### Note state machine

`Raw → Cleaned → Analyzed`, with `CleanFailed` / `ExtractFailed` as terminal-
until-retried states. Each transition is persisted. Each stage is independently
re-runnable from the note's UI, which matters because a model server that was
down when the note was dictated will usually be up later.

`raw` text is **never** overwritten. `body` starts as a copy of `raw` and is
replaced by the cleaned version. This makes cleanup reversible and means a bad
rewrite can never destroy the only record of what was said.

## 5. Data model and storage

New module `src/notes/`.

```rust
pub enum NoteState { Raw, Cleaned, CleanFailed, Analyzed, ExtractFailed }

/// Fixed palette, not free-form hex — keeps notes inside the Deploy Purple
/// design language and keeps `notes.json` validatable.
pub enum NoteColor { Purple, Violet, Amber, Teal, Rose, Slate }

pub struct Note {
    pub id: String,            // millis-since-epoch + counter; no new uuid dep
    pub created: String,       // RFC3339, via chrono (already a dependency)
    pub modified: String,
    pub raw: String,           // verbatim transcript or typed text; never rewritten
    pub body: String,          // display text; == raw until cleanup succeeds
    pub state: NoteState,
    pub color: NoteColor,      // Deploy Purple palette variant
    pub pos: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub open: bool,            // window currently shown
    pub archived: bool,
}

pub struct Task {
    pub id: String,
    pub note_id: String,       // provenance: click a task, get its note
    pub text: String,
    pub done: bool,
    pub created: String,
}
```

Persisted as `notes.json` and `tasks.json` in `Config::config_dir()`, following
`src/ui/history.rs`'s load / parse / corrupt-file-backup pattern exactly —
including renaming an unparseable file to `.corrupt` rather than starting empty
and overwriting it on next save.

Two deliberate departures from `history.rs`:

- **Debounced saves.** `history.rs` rewrites the whole file on every append,
  which is fine for one dictation at a time. Notes are edited per keystroke.
  Saves coalesce on a ~500 ms trailing debounce, with an immediate flush on
  window close and on app shutdown.
- **No entry cap.** `history.rs` caps at 1000 because dictations are disposable.
  Notes are authored content; silently dropping them is wrong. Old notes are
  `archived`, never deleted without user action.

## 6. Capture — the note-mode hotkey

`RecordingConfig` gains `note_hotkey: String` (default: unset, user must pick
one — no default chord will be silently stolen from another app).

`HotkeyEvent` currently carries no identity:

```rust
pub enum HotkeyEvent { RecordStart, RecordStop }
```

It grows a mode:

```rust
pub enum CaptureMode { Inject, Note }
pub enum HotkeyEvent { RecordStart(CaptureMode), RecordStop }
```

Both listeners — `src/hotkey/ll_hook.rs` (Windows) and
`src/hotkey/linux_hotkey.rs` (Linux/evdev) — currently watch exactly one
`HotkeyConfig`. Each must watch two and report which fired. **This is the
least trivial part of the capture work** and is genuinely per-platform; on
Linux it is the evdev matching loop, on Windows the low-level hook predicate.

In the orchestrator, the mode threads through to the transcript sink. The
`TranscriptKind::Final` branches at `src/orchestrator/mod.rs:174`, `:253` and
`:387` dispatch to `do_injection` or `do_note_capture`. Everything upstream —
audio capture, VAD, the backends, tail capture, the recording pill — is shared
untouched.

`src/orchestrator/mod.rs` is at 438 lines against the project's 500-line limit,
so `do_note_capture` and the sink plumbing land in a new
`src/orchestrator/sink.rs` alongside the existing `session.rs`.

The recording pill shows a distinct state (`"note"`) while in note mode so it is
obvious which hotkey was hit. This is a one-line addition to the existing
`ShowIndicator` D-Bus call plus a CSS class in the extension.

## 7. Local inference — `src/llm/`

Beamer's first non-ASR model client.

### Server lifecycle

Beamer supervises `llama-server` child processes rather than requiring the user
to run them. Two processes, one per model, on two loopback ports (defaults
8081 for cleanup, 8082 for extraction). Two processes rather than model
swapping because both fit simultaneously (§3) and swapping a 14.4 GB model
would put a multi-second stall in the middle of the pipeline.

Startup sequence per model:

1. Probe `http://127.0.0.1:<port>/health`. If it answers, use it — this lets a
   user run their own server and lets Beamer survive its own restart without
   respawning.
2. If not, and `llm.manage_server = true`, spawn:

   ```
   llama-server -m <gguf> --port <port> --host 127.0.0.1 \
                -ngl 99 -c <ctx> --jinja
   ```

   with `LD_LIBRARY_PATH` set to the llama.cpp build dir (§2) and
   `GGML_VK_VISIBLE_DEVICES` set to the configured device.

   `--jinja` is required: both models ship embedded chat templates, and
   s1-mini's trained input format (§3) depends on its template being applied
   exactly.
3. Poll `/health` until ready or a timeout elapses — 30 s for s1-mini, **120 s
   for the 26B model**, which must read 14.4 GB from disk and upload it to
   VRAM on a cold start.

Servers are spawned **lazily on first use**, not at Beamer startup.

**Idle shutdown defaults to disabled (`0`).** The earlier draft proposed a
15-minute timeout, sized for a card that was also driving displays. The B60 is
headless and dedicated (§2), so holding 14.9 GB costs nothing anyone else
wants, while a cold reload costs a multi-second stall on the next note. The
setting remains configurable for anyone who wants the VRAM back.

Child processes are killed on Beamer exit; a leaked `llama-server` holding
14.4 GB of VRAM would be a nasty failure mode. The implementation must handle
Beamer being SIGKILLed too — on next start, an already-listening port is
adopted rather than fought over, which step 1 already covers.

All process spawning and the blocking `/health` poll go through
`tokio::task::spawn_blocking`, per the threading rules in
`agent_docs/dioxus_architecture.md`.

### Request shape

`llama-server` exposes an OpenAI-compatible `/v1/chat/completions`. Beamer
already has `reqwest` and a shared `http_client()` in
`src/transcription/mod.rs` — the local calls reuse both, so this is the same
code shape as the existing cloud backends with a different base URL.

**Cleanup** (`llm/cleanup.rs`): sends the exact system prompt and control-line
format specified in §3. Temperature 0. The response is plain text and is used
verbatim. There is no prompt engineering to do here and no room for
improvisation — the format is part of the model's contract.

**Extraction** (`llm/extract.rs`): sends the cleaned `body` and asks for a JSON
array of action items. Temperature 0. Uses llama.cpp's
`response_format: {"type": "json_object"}` grammar constraint, so output is
structurally valid JSON by construction rather than by hope.

```json
{ "tasks": ["call the vet", "send Tuesday's invoice"] }
```

The parser must still tolerate ```json fences — models emit them regardless of
instructions, and the grammar constraint does not apply to every server
version.

### Failure handling

The governing rule: **a failure never costs the user words.**

- Cleanup returns **empty** → this is *success*, not failure (§3). Filler-only
  speech correctly normalizes to nothing. `body` stays equal to `raw`, state
  becomes `Cleaned`, and the note is flagged "nothing to clean" rather than
  blanked.
- Cleanup errors (network, non-2xx, timeout) → `body` stays equal to `raw`,
  state becomes `CleanFailed`, note shows a retry affordance. Extraction still
  runs, against `raw`.
- Extraction fails → state becomes `ExtractFailed`, no tasks created, retry
  available. The note is unaffected.
- Server unreachable, model file missing, GPU busy, spawn failed → identical
  handling. Note creation itself never depends on any of it.

Errors are logged through the existing `StatusLog` so they surface in the app
rather than only in `RUST_LOG`.

### Trigger policy

- **Dictated notes:** cleanup runs automatically (you asked for correction, and
  you cannot proofread speech as you produce it). Extraction runs after it.
- **Typed notes:** neither runs automatically. Rewriting text someone
  deliberately typed is presumptuous — and s1-mini is a *transcript* normalizer,
  so feeding it prose that was never spoken is outside its training
  distribution. An explicit "Enhance" button runs both.

## 8. Sticky windows — optimized for Mutter

One Dioxus window per open note, created with
`window.new_window(dom, cfg).await` — the identical pattern already used for
the splash and pill windows at `src/ui/app_setup.rs:102` and `:194`. New file
`src/ui/sticky.rs`.

Window configuration: decorations off with a Deploy Purple custom titlebar
(the main window already does this), **always-on-top off** (explicit user
decision), initial size restored from the note.

A `Signal<HashMap<NoteId, DesktopContext>>` registry maps note id to live
window so notes cannot be double-opened and can be closed programmatically.

### Position persistence on Wayland

Wayland gives clients no control over their own window position, and Mutter
does not implement `wlr-layer-shell`. `src/ui/linux_integration.rs:6` already
documents hitting this wall with the recording pill.

Beamer's own shell extension is the way around it. `app.beamer.FocusProvider`
gains one method:

```xml
<method name="PlaceWindow">
  <arg type="s" direction="in"  name="title"/>
  <arg type="i" direction="in"  name="x"/>
  <arg type="i" direction="in"  name="y"/>
  <arg type="b" direction="in"  name="all_workspaces"/>
  <arg type="b" direction="out" name="ok"/>
</method>
```

Implementation in the extension: enumerate `global.get_window_actors()`, match
on window title, call `meta_window.move_frame(true, x, y)` and, when
`all_workspaces` is set, `meta_window.stick()` so the note follows across
workspaces — which is what a sticky note ought to do, and is only reachable
from inside the shell.

Each sticky window's title is `Beamer Note <note-id>`, giving the extension an
unambiguous match.

Beamer cannot observe the Wayland map event directly, so `PlaceWindow` is
called on a short delay after `new_window()` returns and **retried with backoff
up to ~2 s** until the extension reports a match — the window may not exist
from Mutter's point of view at the moment the call is made. A miss after that
is logged and abandoned; the note is simply where Mutter put it.

Position is captured back on window move for persistence. The extension
version constant bumps to 5, and `shell_indicator.rs`'s existing
`GetVersion` gate means an out-of-date extension degrades to "notes appear
wherever Mutter puts them" rather than breaking.

**Reminder:** GNOME extensions do not hot-reload on Wayland. Testing extension
changes requires a full log out and back in.

### Windows

The same `sticky.rs` compiles and runs on Windows, where `WindowBuilder`
position is honored natively and the `PlaceWindow` call is simply not made. No
renderer abstraction or trait is introduced — the difference is one
`#[cfg]`-gated call, and inventing a trait for that would be architecture for
its own sake.

## 9. In-app pages

`Page` (in `src/ui/app.rs`) gains `Notes` and `Tasks`, with sidebar entries
beside History.

- **`src/ui/notes_page.rs`** — a board of note cards in a grid, in the style of
  `vixalien/sticky`'s all-notes view: color swatch, first lines of `body`,
  relative timestamp, task count badge. Click pops out the sticky window. Text
  search over `raw` and `body`. Archive toggle.
- **`src/ui/tasks_page.rs`** — flat checkbox list grouped by source note, with
  a done/undone filter and click-through to the originating note. Checking a
  task writes through to `tasks.json` on the same debounce.

Both reuse `Card`, `Toggle` and the other primitives in
`src/ui/components.rs`.

## 10. Config additions

```toml
[recording]
note_hotkey = ""              # unset by default

[llm]
enabled = true
manage_server = true          # false = connect only, never spawn
llama_server_path = "/home/berkley/Programming/llama.cpp/build/bin/llama-server"
llama_lib_dir = "/home/berkley/Programming/llama.cpp/build/bin"
vulkan_device = 0             # B60; confirm against `llama-server --list-devices`
idle_shutdown_minutes = 0     # 0 = never; B60 is headless and dedicated

[llm.cleanup]                 # "S1-mini" by "Superwhisper"
model_path = "…/s1-mini-q4_k_m.gguf"
port = 8081
ctx = 8192
styling = "semi-formal"       # casual | semi-casual | semi-formal | formal
structure = "lists"           # prose | lists
context = "general"           # general | email

[llm.extract]                 # google/gemma-4-26B-A4B-it QAT q4_0
model_path = "…/gemma-4-26B_q4_0-it.gguf"
port = 8082
ctx = 8192

[notes]
all_workspaces = true         # Mutter: stick() notes across workspaces
default_color = "purple"
```

Model files are **not** downloaded by Beamer. They are fetched manually (or by
a small documented script committed alongside the spec) and their paths
configured. Adding a downloader is a separate feature with its own progress UI,
resumability and disk-space concerns — and at 14.4 GB it is not a trivial one.

`agent_docs/config_schema.md` must be updated to match.

## 11. Testing

Unit-testable without a compositor or a GPU:

- **`notes::store`** — serialize/deserialize round-trip; corrupt-file preserved
  as `.corrupt` and a fresh store returned; debounce coalescing (many edits →
  one write); archive does not delete.
- **`llm` extraction parsing** — clean JSON; JSON wrapped in ```json fences;
  malformed JSON → error, not panic; `{"tasks": []}` → zero tasks, not an error.
- **`llm` cleanup contract** — an **empty** cleanup response is treated as
  success with `body` left equal to `raw` (§3), *not* as a failure. A test must
  pin this, because the intuitive implementation gets it backwards.
- **s1-mini request shape** — the system prompt is byte-identical to §3 and the
  control line only ever carries trained values. A test should reject any
  attempt to send an out-of-set styling/structure/context value.
- **State machine** — every stage failure leaves `raw` intact and `body`
  non-empty.
- **Sink routing** — `HotkeyEvent::RecordStart(CaptureMode::Note)` reaches
  `do_note_capture` and not `do_injection`.

Requires a live model server (integration, run manually or behind a feature
flag): end-to-end cleanup and extraction against real `llama-server` instances.

Manual only: sticky window rendering, Mutter placement via `PlaceWindow`,
cross-workspace stick, hotkey capture on both platforms.

## 12. File layout

Against the project's 500-lines-per-file constraint:

| File | Purpose |
|---|---|
| `src/notes/mod.rs` | `Note`, `NoteState`, store, debounced persistence |
| `src/notes/task.rs` | `Task`, task store |
| `src/llm/mod.rs` | client, config, shared request plumbing |
| `src/llm/server.rs` | `llama-server` spawn / health / idle shutdown |
| `src/llm/cleanup.rs` | stage 1 prompt + call |
| `src/llm/extract.rs` | stage 2 prompt + call + JSON parsing |
| `src/orchestrator/sink.rs` | `CaptureMode`, `do_note_capture` |
| `src/ui/sticky.rs` | sticky note window component + registry |
| `src/ui/notes_page.rs` | all-notes board |
| `src/ui/tasks_page.rs` | tasks list |

Modified: `src/hotkey/mod.rs`, `ll_hook.rs`, `linux_hotkey.rs`,
`src/orchestrator/mod.rs`, `src/ui/app.rs`, `src/config/mod.rs`,
`src/ui/shell_indicator.rs`, `extension/beamer-focus@beamer.app/extension.js`.

New docs: `agent_docs/local_inference.md`. Updated:
`agent_docs/config_schema.md`, `agent_docs/dioxus_architecture.md`.

New non-code deliverables, both required by §3.3 rather than optional:
`licenses/` holding both model licenses, and an About/credits surface in the
settings UI carrying `"S1-mini" by "Superwhisper"` verbatim. `README.md` gains
the same attribution.

## 13. Risks

- **Every performance figure in this spec is an estimate.** None was
  benchmarked. The reasoning is sound (MoE reads ~2 GB/token vs ~16 GB for a
  dense 27B on a bandwidth-bound card) but reasoning is not measurement.
  **Benchmarking both models on the B60 via Vulkan is implementation step 1**,
  before any Rust is written. If stage 1 does not land in roughly a second on a
  typical note, the "watch the sticky tidy itself" experience does not exist
  and the fast path needs rethinking.
- **Vulkan MoE performance on Intel is the largest unknown.** llama.cpp's
  Vulkan backend is well exercised for dense transformers; MoE expert routing
  is less travelled. If gemma-4-26B-A4B underperforms its parameter count,
  the fallback ladder is `empero-ai/Qwen3.8-9B-Distill-GGUF` (dense, ~9.5 GB)
  then `NVIDIA-Nemotron-3.5-Lightning-30B-A3B`.
- **Gemma 4 licensing must be confirmed at download.** Google's own repos are
  tagged `license:apache-2.0`, but some third-party derivatives carry
  `license:gemma`. Read the LICENSE in the repo actually downloaded, as was
  done for s1-mini.
- **The s1-mini naming attribution is a shipping requirement** (§3.3), not a
  nicety. It is easy to forget and it is a licence term.
- **s1-mini's input format is unforgiving.** The publisher documents that a
  reworded system prompt or an out-of-set control value produces hallucinated
  or garbled output. It must be treated as a wire format with a test pinning
  it, not as a prompt someone may later "improve".
- **Vulkan device indices are not stable** across driver updates or card
  changes. `GGML_VK_VISIBLE_DEVICES=0` may not always mean the B60. The
  implementation must log the device llama-server actually selected at startup
  so a mismatch is visible rather than mysterious.
- **14.4 GB cold start.** First use after boot reads 14.4 GB from disk. With
  idle shutdown disabled this happens once per session, but the first note of
  the day will wait on it. Beamer should surface "loading model" state rather
  than appearing hung — and note creation itself still must not block.
- **Two extra hotkey listeners** are the most likely source of platform-
  specific bugs, particularly on the evdev path.
- **Extension changes require a re-login** to test, which makes iteration on
  `PlaceWindow` slow. Budget for it.

## 14. Out of scope

Meeting/system-audio capture; local ASR; model downloading from within Beamer;
rich text; sync or sharing; recurring tasks, due dates or reminders; export to
external task managers; note templates.

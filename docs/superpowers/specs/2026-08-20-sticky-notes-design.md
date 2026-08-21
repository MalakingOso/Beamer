# Sticky Notes with On-Device AI — Design

**Date:** 2026-08-20
**Status:** Draft for review
**Target platform:** GNOME Wayland / Mutter (primary). Windows kept compiling, not optimized for.

## 1. Summary

A second global hotkey dictates into a **sticky note** instead of injecting text
into the focused field. The note window appears immediately with the raw
transcript. Two on-device model passes then run in the background: a fast
cleanup pass rewrites the transcript into properly punctuated prose in place,
and a slower analysis pass proposes action items as **suggestions on the note**,
which you accept into a global **Tasks** page or dismiss.

No cloud service is involved in the AI passes. Speech-to-text continues to use
the existing cloud backends, because that path is already fast and already
built.

### Goals

- Dictate a thought and have it land on the desktop as a note, without
  interrupting whatever is focused.
- The note reads like written English, not like a transcript.
- Action items buried in the note surface as checkable tasks — and, more
  importantly, things that merely *sound* like tasks do not.
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
| **llama.cpp router server** | This build has a multi-model router: `--models-dir`, `--models-max` (default 4), `--models-autoload`, with LRU eviction at the cap. HTTP: `GET /models`, `POST /models/load`, `POST /models/unload`. | `tools/server/server-models.cpp`, `server.cpp:164-166` |
| **llama.cpp build age** | Local checkout is `e97492369`, **2026-04-13** — four months stale as of this spec. | `git log -1` |
| Desktop | GNOME Shell **50.1**, Wayland session, Mutter 18 | `gnome-shell --version`, `$XDG_SESSION_TYPE` |
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
| License | Apache 2.0 **plus an additional naming term** (see “Attribution obligation” below) |

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

### Stage 2 — task extraction: `google/gemma-4-E4B-it-qat-q4_0-gguf`

**This is a starting point chosen to be replaced by measurement, not a final
answer.** See “Choosing the extraction model by measurement” below.

| Property | Value |
|---|---|
| File | `gemma-4-E4B_q4_0-it.gguf`, **5.15 GB** |
| Architecture | Gemma 4 "E" variant, ~4B effective parameters |
| Quantization | **QAT** — quantization-aware trained, published by Google |
| License | Apache-2.0 per the Hub tag (verify at download) |
| Context | Beamer uses 8,192 |

The `mmproj` file (991 MB vision projector) is **not** downloaded — Beamer's
use is text-only.

#### Why not the 26B MoE that the previous draft specified

An earlier draft chose `gemma-4-26B-A4B-it` (14.44 GB) on the argument that
MoE decode is cheap: it reads only its 8 active experts of 128, roughly
2.2 GB/token, against ~5.2 GB/token for this dense E4B. By that measure the
26B is genuinely the *faster* model despite being three times the size on
disk, and that argument still holds.

It was rejected anyway, for two reasons:

1. **VRAM on the B60 is not free.** The earlier draft claimed it was, because
   the card is headless. That ignored the fact that this machine runs other
   GPU work on the B60 — a vLLM/XPU stack among it. Permanently parking
   14.9 GB there so Beamer can serve a handful of short requests a day is a
   bad trade against work that actually needs the card.
2. **The job is small, and the hard part isn't capacity.** The difficulty in
   stage 2 is *judgment* — deciding that something is not a task (§7.3) — and
   the design now handles that with a confirmation step rather than by
   assuming a larger model will be right more often.

At 5.15 GB + 0.46 GB, Beamer leaves roughly 17 GB of the B60 free for
everything else.

#### The model ladder

If measurement (below) shows E4B's precision is inadequate, promote in this
order. All are official Google QAT builds under Apache-2.0, so promotion is a
config change and a download — no code.

| Rung | Model | On disk | Read/token |
|---|---|---|---|
| 1 (default) | `gemma-4-E4B-it` q4_0 | 5.15 GB | ~5.2 GB |
| 2 | `gemma-4-12B-it` QAT UD-Q4_K_XL | 6.72 GB | ~6.7 GB |
| 3 | `gemma-4-26B-A4B-it` q4_0 | 14.44 GB | ~2.2 GB (MoE) |

Note rung 3 is both the largest and the fastest. If latency rather than
quality turns out to be the binding constraint, skip rung 2.

### VRAM budget

| Item | Size |
|---|---|
| `gemma-4-E4B_q4_0-it.gguf` | 5.15 GB |
| `s1-mini-q4_k_m.gguf` | 0.46 GB |
| **Weights total** | **5.61 GB** |
| Available on B60 | 22.7 GB |
| **Left free for other work on the card** | **~17 GB** |

Both models stay resident simultaneously. At rung 3 the total would be
14.90 GB, leaving ~7.8 GB.

### Choosing the extraction model by measurement

The right model for stage 2 is an empirical question about *this user's*
dictation, and the spec deliberately does not assert an answer.

**Corpus.** Roughly 50 real notes captured through Beamer's own note hotkey,
hand-labelled with the tasks each should produce — which for many notes is
**none**. Deliberately over-sample notes containing no tasks; those are the
cases where extraction models fail, and a corpus of only task-bearing notes
would hide the failure this feature most needs to avoid.

**Metrics.** **Precision is primary** — under the strict policy in §7.3 a
fabricated task is worse than a missed one. Recall is secondary. Report both,
plus per-example diffs so regressions are legible rather than a moving number.

**Harness.** A standalone binary under `src/bin/` that runs the corpus against
a configured endpoint and prints the scores. There is precedent: `src/bin/
voxtral_test.rs` and `src/bin/ws_test.rs` already exist as backend probes.

**Free ongoing labels.** Every suggestion the user dismisses is a labelled
negative, and every one accepted is a labelled positive (§5). The corpus grows
by using the feature.

### Alternatives considered and rejected

| Candidate | Size | Why not |
|---|---|---|
| `NVIDIA-Nemotron-3.5-Lightning-30B-A3B` | ~17 GB Q4 | Closest rival — 3B active, GGUF published by **ggml-org itself**. Rejected on three counts: `license: other` vs Apache-2.0; post-training imatrix quants vs QAT; and it is a **mamba2 hybrid**, and SSM kernels are the least-mature path in llama.cpp's Vulkan backend. A performance surprise on Intel is likely enough to matter. **Documented as the primary A/B alternative.** |
| `ornith-ai/Ornith-1.5-35B-A3B-GGUF` | **21.7 GB** Q4_K_M | Does not fit alongside stage 1 with usable KV cache. Also 2 days old at time of writing, from an unproven org. |
| `Qwen3.8-27B` GGUF | ~16 GB Q4 | Dense. Same VRAM, ~3–4× slower decode than a comparable MoE. |
| `google/gemma-4-31B-it` | ~18 GB Q4 | Dense, larger, slower, no QAT build. |
| `gemma-4-26B-A4B-it` QAT | 14.44 GB | Fastest option and strong, but parks 15 GB on a card this machine needs for other GPU work. Kept as rung 3 of the ladder (§3), not the default. |
| `empero-ai/Qwen3.8-9B-Distill-GGUF` | ~9.5 GB Q8 | Viable, but not QAT and outside the Gemma 4 ladder, so promoting to it means re-running the eval from scratch rather than swapping a path. Keep in reserve. |
| `LiquidAI/LFM2.5-2.6B-GGUF` | ~2 GB | Non-QAT and weaker than a same-size Gemma 4 QAT build. Reasonable rung-0 if even 5 GB proves too much. |
| `unsloth/…-qat-GGUF` UD-Q4_K_XL | 14.25 GB | Unsloth's dynamic K-quant of the same QAT weights. Arguably better bit allocation than legacy q4_0, but q4_0 is the scheme QAT actually targeted. Worth A/B testing; not the default. |
| `Cactus-Compute/needle2` | — | On-device tool-calling specialist, but `cactus-needle` format — a whole second runtime. |
| DeepSeek-V4 family | — | **No `deepseek_v4` arch in this llama.cpp build.** Cannot run. |
| Any `mtp-*.gguf` draft weights | — | MTP tensors are explicitly ignored by this build (§2). |

### Attribution obligation

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
llm::extract  (gemma-4-E4B)     ──► Task rows created as **Suggested**
      │                                  state: Analyzed
      ▼
suggestion chips appear on the sticky
      │
      ▼
user accepts ──► Tasks page      user dismisses ──► retained as a
                                                     labelled negative
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

/// A suggestion's lifecycle. Dismissed rows are RETAINED, never deleted —
/// they are the labelled negatives that grow the eval corpus (§3).
pub enum TaskStatus { Suggested, Accepted, Dismissed }

pub struct Task {
    pub id: String,
    pub note_id: String,       // provenance: click a task, get its note
    pub text: String,          // normalized imperative, e.g. "Call the vet"
    pub evidence: String,      // exact span of the note that triggered it
    pub confidence: f32,       // 0.0-1.0, as reported by the model
    pub status: TaskStatus,
    pub done: bool,            // only meaningful when status == Accepted
    pub created: String,
    pub decided: Option<String>, // when accepted/dismissed; the eval signal
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
  `archived`, never deleted without user action. `Dismissed` tasks are likewise
  retained rather than deleted — they are training signal, not litter.

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

### Runtime: llama.cpp, rebuilt — not Ollama

**Decision: keep llama.cpp, but rebuild from current master.** The local
checkout is from 2026-04-13 and is four months stale.

Ollama was evaluated and rejected. Its standard release does not support Intel
Arc; Intel's supported path is the **IPEX-LLM** fork of Ollama, which is retired
software — Intel ceased development after PyTorch 2.8 and maintenance ended in
March 2026, and it is explicitly off-limits on this machine. The remaining
option is an unofficial community Vulkan build of Ollama, which is strictly
worse than the working, already-compiled Vulkan llama.cpp sitting on disk.

Ollama's genuine advantage was model lifecycle management — load on demand,
keep-alive, automatic unload. **The llama.cpp router server now provides that
natively**, which removes the last reason to consider it.

Reasons to rebuild before implementation:

- Four months of Vulkan backend and Gemma 4 / Qwen 3.5 fixes.
- MTP support may have landed; this build still says *"NextN/MTP tensors are
  currently ignored"* (§2). If it has, the ladder's rung 3 gets meaningfully
  faster.
- The router server itself should be exercised on a current build rather than an
  April snapshot.

Rebuild with Vulkan as currently configured — do **not** switch to SYCL. Vulkan
needs no oneAPI environment sourced, which is decisive for a process Beamer
spawns from a desktop session.

### Server lifecycle: one router process

The earlier draft had Beamer supervising two `llama-server` processes on two
ports with its own per-model idle timers. The router makes that unnecessary.

**One process, both models, addressed by name:**

```
llama-server --models-dir ~/models/beamer --models-max 2 \
             --host 127.0.0.1 --port 8080 -ngl 99 -c 8192 --jinja
```

with `LD_LIBRARY_PATH` set to the llama.cpp build dir (§2) and
`GGML_VK_VISIBLE_DEVICES` set to the B60's Vulkan index.

Requests select the model the ordinary OpenAI way:

```json
{ "model": "s1-mini-q4_k_m", "messages": [...] }
```

The router loads a model on first request (`--models-autoload`) and evicts the
least-recently-used one when `--models-max` is exceeded. `--models-max 2` keeps
both resident, which is what the 5.61 GB budget (§3) assumes.

**Control surface** (`server.cpp:164-166`):

| Endpoint | Use |
|---|---|
| `GET /models` | Health and per-model load state — replaces the `/health` polling of the earlier design |
| `POST /models/load` | Warm S1-mini at startup so the first note is not slow |
| `POST /models/unload` | Explicitly release the extraction model when idle |

**Asymmetric residency is now explicit rather than inferred.** The router's
eviction is LRU-at-a-cap, not time-based, so Beamer implements the idle policy
itself with one HTTP call:

- **S1-mini** — `POST /models/load` at Beamer startup, never unloaded. It is
  462 MB and it is the stage the user watches.
- **Extraction model** — loaded on demand by the router; Beamer sends
  `POST /models/unload` after `idle_shutdown_minutes` with no extraction. It is
  5.15 GB and nobody is waiting on it.

This is a large simplification: one child process instead of two, one port
instead of two, `GET /models` instead of hand-rolled health polling, and the
load/unload logic reduced to two HTTP calls.

**Startup sequence:**

1. `GET http://127.0.0.1:8080/models`. If it answers, use it — a user running
   their own router is supported, and Beamer survives its own restart without
   respawning.
2. If not, and `llm.manage_server = true`, spawn the router as above.
3. Poll `GET /models` until it answers, or time out (30 s — the router starts
   without loading any model, so this no longer waits on a multi-GB read).
4. `POST /models/load` for S1-mini.

`--jinja` is required: both models ship embedded chat templates, and S1-mini's
trained input format (§3) depends on its template being applied exactly.

The child process is killed on Beamer exit; a leaked router holding several GB
of VRAM would be a nasty failure mode. Step 1 already handles the SIGKILL case
by adopting an already-listening port rather than fighting it.

All process spawning and blocking HTTP polls go through
`tokio::task::spawn_blocking`, per `agent_docs/dioxus_architecture.md`.

### Request shape

`llama-server` exposes an OpenAI-compatible `/v1/chat/completions`. Beamer
already has `reqwest` and a shared `http_client()` in
`src/transcription/mod.rs` — the local calls reuse both, so this is the same
code shape as the existing cloud backends with a different base URL.

**Cleanup** (`llm/cleanup.rs`): sends the exact system prompt and control-line
format specified in §3. Temperature 0. The response is plain text and is used
verbatim. There is no prompt engineering to do here and no room for
improvisation — the format is part of the model's contract.

**Extraction** (`llm/extract.rs`): sends the cleaned `body` and asks the model
to *judge* what, if anything, is a task. Temperature 0. Uses llama.cpp's
`response_format: {"type": "json_object"}` grammar constraint, so output is
structurally valid JSON by construction rather than by hope.

```json
{ "tasks": [
    { "text": "Call the vet", "evidence": "I need to call the vet about Milo", "confidence": 0.93 }
] }
```

Every task carries the **exact span** of the note that produced it. This is not
decoration: it is what makes a false positive visible to the user in one glance
and makes the prompt debuggable when it misfires. A task with no traceable
evidence is a fabrication, and the parser rejects any whose `evidence` is not a
substring of the note.

The parser must still tolerate ```json fences — models emit them regardless of
instructions, and the grammar constraint does not apply to every server
version. `{"tasks": []}` is a normal, frequent, successful response.

### 7.3 What counts as a task

This is the hard part of the whole feature, and it is a **precision** problem,
not an extraction problem. An extraction-shaped prompt ("list the action items")
biases the model toward producing output, which is exactly the failure to
avoid: a note that is pure venting must yield nothing.

**Policy: strict — first-person commitments only.** A task is only created for
a concrete, future action the speaker themselves has committed to. Everything
else stays as note text.

The system prompt enumerates the negative categories explicitly, because naming
them is what suppresses them:

| Not a task | Example |
|---|---|
| Completed or past action | "I called the vet yesterday" |
| Someone else's action | "Sarah is sending the invoice" |
| Hypothetical or conditional | "if the build fails we'd roll back" |
| Opinion, venting, emotion | "I'm so done with this project" |
| Observation or fact | "the API returns 500 on empty payloads" |
| Vague aspiration or idea | "we should think about caching", "it'd be cool to have dark mode" |
| Rhetorical question | "why do I even bother" |

Aspirations are deliberately excluded. They are the largest ambiguous class,
and admitting them is what turns a task list into a graveyard of vague
intentions. The policy can be loosened later; a list nobody trusts cannot be
un-poisoned.

Three further prompt requirements:

1. **State that empty is normal.** "Most notes contain no tasks. Returning an
   empty list is the correct and common answer. When uncertain, return
   nothing." Without this, models reliably invent one.
2. **Few-shot with hard negatives.** At least two exemplars are notes that
   contain no tasks and correctly return `{"tasks": []}`. Positive-only
   exemplars teach the model that output is always expected.
3. **Bias toward omission**, stated explicitly, because the confirmation step
   (below) makes a missed task cheap to notice and a fabricated one expensive
   to trust.

### 7.4 Suggestions, not facts

Extraction never writes to the Tasks page directly. Tasks are created with
`status: Suggested` and surface as chips on the sticky note, each showing its
text and its evidence span. One click accepts, one dismisses.

This is the structural answer to the precision problem, and it does three
things at once:

- A false positive costs one click instead of quietly polluting the task list.
- It lowers the model quality bar, which is what makes a 5 GB model a
  reasonable default instead of a 15 GB one.
- Every decision is a **labelled example**. Dismissed rows are retained (§5),
  so the eval corpus in §3 grows every time the feature is used.

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

### Why a client cannot know where it is on Wayland

This deserves stating precisely, because the intuitive fix — "just read the
window position and save it" — is not available, and the API that looks like it
works lies.

**1. There is no protocol for it.** Core Wayland and `xdg-shell` expose no way
for a client to learn or set its absolute position. `xdg_surface.set_window_geometry`
concerns the surface's own bounds within its buffer (shadows, decorations), not
its place on screen. This is a deliberate design decision: the compositor owns
placement, and clients are not told about global coordinates at all.

**2. The toolkit API returns a plausible lie.** `tao::Window::outer_position()`
looks usable — it returns `Ok`, not `Err(NotSupportedError)`. Reading
`tao-0.34.8/src/platform_impl/linux/window.rs:465` shows why that is worse than
an error: it returns a **cached atomic**, populated at
`window.rs:344-352` from GDK's `frame_extents()` on each `configure_event`.
Under Wayland, GDK has no global coordinates to report, so the cache holds
`(0, 0)` forever. Code that trusts it silently persists the wrong answer.
**Beamer must never call `outer_position()` on Wayland.**

**3. The official fix exists but is not usable here yet.**
`xx-session-management-v1` is the Wayland protocol designed for exactly this
problem, and its design is itself the clearest statement of the rule: the
application **never learns coordinates**. It requests an opaque session ID,
gives each toplevel a name, and later asks the compositor to restore that named
window — the compositor supplies the position, not the app. Qt 6.10 implements
it, and SDL added support in March 2026. Mutter's implementation has been gated
behind `MUTTER_DEBUG_SESSION_MANAGEMENT_PROTOCOL=1`; scanning
`libmutter-18.so.0` and `/usr/bin/gnome-shell` on this machine (GNOME 50.1)
found no exported `xx_session_management_v1` symbol, so it cannot be relied on
today.

**4. Real apps simply give up.** `vixalien/sticky` — the GTK4 sticky-notes app
this design was modelled on — persists `width`, `height` and `open`, and **no
x/y at all**. Its notes reappear wherever Mutter decides. That is the honest
state of the art for a well-built Wayland notes app, and it is the behaviour
Beamer would inherit by doing nothing.

**5. Beamer can do better only because it already ships a shell extension.**
Code running inside GNOME Shell is not a Wayland client and is not subject to
any of the above: `Meta.Window.get_frame_rect()` returns real coordinates and
`Meta.Window.move_frame()` sets them. Most apps cannot justify shipping an
extension for this. Beamer already ships one for text injection and the
recording pill, so the marginal cost is two D-Bus methods.

When Mutter exposes `xx-session-management-v1` without a debug flag, that
becomes the portable path and this dependency can be dropped. Until then, the
extension is the only mechanism that works on this machine.

### Position persistence via the shell extension

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
- **`src/ui/tasks_page.rs`** — flat checkbox list of **accepted** tasks only,
  grouped by source note, with a done/undone filter and click-through to the
  originating note. Checking a task writes through to `tasks.json` on the same
  debounce. Dismissed tasks never appear here; they are retained in storage as
  eval data, viewable only from a debug surface.

Suggestion chips live on the sticky note itself (`sticky.rs`), not on either
page: each shows the proposed task text with its evidence span, and an
accept/dismiss pair. A note with pending suggestions shows a count badge on its
card in the notes board.

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
models_dir = "/home/berkley/models/beamer"
port = 8080                   # one router process serves both models
models_max = 2
vulkan_device = 0             # B60; confirm against `llama-server --list-devices`

[llm.cleanup]                 # "S1-mini" by "Superwhisper"
model = "s1-mini-q4_k_m"      # router model name, not a path
idle_shutdown_minutes = 0     # resident: latency-critical, only 462 MB
styling = "semi-formal"       # casual | semi-casual | semi-formal | formal
structure = "lists"           # prose | lists
context = "general"           # general | email

[llm.extract]                 # google/gemma-4-E4B-it QAT q4_0 (ladder rung 1)
model = "gemma-4-E4B_q4_0-it"
idle_shutdown_minutes = 5     # POST /models/unload when idle: 5 GB, nobody waits
min_confidence = 0.5          # below this, the suggestion is not shown at all

[notes]
all_workspaces = true         # Mutter: stick() notes across workspaces
default_color = "purple"
```

Model files are **not** downloaded by Beamer. They are fetched manually (or by
a small documented script committed alongside the spec) and their paths
configured. Adding a downloader is a separate feature with its own progress UI,
resumability and disk-space concerns.

`agent_docs/config_schema.md` must be updated to match.

## 11. Testing

Unit-testable without a compositor or a GPU:

- **`notes::store`** — serialize/deserialize round-trip; corrupt-file preserved
  as `.corrupt` and a fresh store returned; debounce coalescing (many edits →
  one write); archive does not delete.
- **`llm` extraction parsing** — clean JSON; JSON wrapped in ```json fences;
  malformed JSON → error, not panic; `{"tasks": []}` → zero tasks, not an error.
- **Evidence validation** — a task whose `evidence` is not a substring of the
  note is rejected as a fabrication rather than shown to the user.
- **Confidence threshold** — suggestions below `min_confidence` are dropped.
- **Suggestion lifecycle** — accepting moves a task to the Tasks page;
  dismissing retains the row with `status: Dismissed` and a `decided`
  timestamp. A test must assert dismissal does **not** delete, since the eval
  corpus depends on it.
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
| `src/llm/prompts.rs` | the s1-mini wire format and the task-policy prompt, with its few-shot exemplars |
| `src/bin/task_eval.rs` | eval harness: runs the labelled corpus, reports precision/recall |
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
- **Task/not-task precision is the feature's central risk**, and no model
  choice eliminates it. The mitigations are structural — strict policy,
  enumerated negatives, hard-negative exemplars, evidence spans, a confidence
  floor, and above all the confirmation step. Expect the first prompt to
  over-trigger and expect to iterate on it against the corpus (§3).
- **A 4B model may not hold the strict policy.** Suppressing output is harder
  for small models than producing it; E4B may propose tasks for aspirations
  and hypotheticals no matter how the prompt is worded. That is precisely what
  the eval corpus is for, and why the ladder to 12B and 26B-A4B is documented
  as a config change rather than a rewrite.
- **Vulkan MoE performance on Intel is untested** and matters only if the
  ladder reaches rung 3. llama.cpp's Vulkan backend is well exercised for dense
  transformers; MoE expert routing is less travelled.
- **Gemma 4 licensing must be confirmed at download.** Google's own repos are
  tagged `license:apache-2.0`, but some third-party derivatives carry
  `license:gemma`. Read the LICENSE in the repo actually downloaded, as was
  done for s1-mini.
- **The s1-mini naming attribution is a shipping requirement** (§3), not a
  nicety. It is easy to forget and it is a licence term.
- **s1-mini's input format is unforgiving.** The publisher documents that a
  reworded system prompt or an out-of-set control value produces hallucinated
  or garbled output. It must be treated as a wire format with a test pinning
  it, not as a prompt someone may later "improve".
- **Vulkan device indices are not stable** across driver updates or card
  changes. `GGML_VK_VISIBLE_DEVICES=0` may not always mean the B60. The
  implementation must log the device llama-server actually selected at startup
  so a mismatch is visible rather than mysterious.
- **Cold start.** First use after boot reads 5.6 GB from disk (14.9 GB at
  ladder rung 3). With idle shutdown disabled this happens once per session,
  but the first note of the day waits on it. Beamer should surface a "loading
  model" state rather than appearing hung — and note creation itself still must
  not block.
- **Beamer shares the B60 with other work on this machine.** The 5.6 GB default
  is sized to stay out of the way. Anyone promoting to rung 3 should know they
  are claiming 15 GB of that card for as long as Beamer runs.
- **Two extra hotkey listeners** are the most likely source of platform-
  specific bugs, particularly on the evdev path.
- **Extension changes require a re-login** to test, which makes iteration on
  `PlaceWindow` slow. Budget for it.

## 14. Out of scope

Meeting/system-audio capture; local ASR; model downloading from within Beamer;
rich text; sync or sharing; recurring tasks, due dates or reminders; export to
external task managers; note templates.

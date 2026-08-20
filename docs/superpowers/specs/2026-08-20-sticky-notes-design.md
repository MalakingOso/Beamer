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
| Vulkan driver | Mesa 26.1.7 (kisak-mesa PPA), `DRIVER_ID_INTEL_OPEN_SOURCE_MESA`, both Arc cards enumerated | `vulkaninfo --summary` |
| llama.cpp | built at `/home/berkley/Programming/llama.cpp/build/bin/`, version 8782 (`e97492369`) | `llama-server --version` |
| llama.cpp backend | **Vulkan** (`GGML_VULKAN:BOOL=ON`, `GGML_SYCL:BOOL=OFF`, `GGML_CUDA:BOOL=OFF`) | `build/CMakeCache.txt` |
| llama.cpp arch support | `qwen3`, `qwen35`, `qwen35moe`, `qwen3moe`, `qwen3next`, `qwen3vl` | `src/llama-arch.cpp` |
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

Two models, two jobs. Splitting them is what makes the fast path fast.

### Stage 1 — cleanup: `superwhisper/s1-mini-GGUF`

- 751.6M parameters, Qwen3-0.6B finetune, Apache-adjacent (`license: other` —
  **must be read before shipping**, see Risks).
- Trained specifically for ASR post-processing: text normalization, inverse
  text normalization, punctuation, truecasing, dictation cleanup.
- Official GGUF published by the same org, tagged `llama.cpp`.
- Quantization: **Q8_0** (~800 MB). At this size the memory saved by Q4 is
  irrelevant and the job is verbatim-fidelity text rewriting, where quantization
  damage shows up directly as wrong words.
- Expected latency for a 200-word note: well under one second *(estimate)*.

This model is doing the job it was built for. A general instruct model prompted
to "add punctuation" is strictly worse here and slower.

### Stage 2 — task extraction: `LiquidAI/LFM2.5-2.6B-GGUF`

- 2.6B parameters, 421K downloads, built and quantized for on-device use,
  GGUF-native.
- Job: read the cleaned note, emit a JSON array of action items.
- Quantization: **Q6_K** (~2.2 GB). VRAM is not scarce here, and extraction
  quality is more sensitive to quantization than cleanup throughput is.
- Expected latency: 1–3 s for a short JSON output *(estimate)*.

**Upgrade path if extraction quality disappoints:** `empero-ai/Qwen3.8-9B-Distill-GGUF`
(9B, tagged `reasoning` and `function-calling`, Apache-2.0) at Q4_K_M ≈ 5.5 GB.
Still comfortable on either card. Slower, considerably more capable. The model
id is a config field precisely so this swap costs no code.

Also noted and rejected for now: `Cactus-Compute/needle2` (on-device function
calling, but `cactus-needle` format rather than GGUF — a new runtime), and the
Qwen3.8-27B family (overkill; the B60 could run it, but a 27B model spun up to
extract two to-dos is absurd).

### GPU assignment

**Default to the B570 (`card1`, 10 GB), not the B60.** Reasons:

1. Both models together are under 3 GB; 10 GB is ample.
2. The B60 has a documented wedge history (2026-04-29, 2026-05-07) and is the
   card reserved for heavier work in other projects. Leaving it free avoids
   contention and avoids Beamer being the process that wedges it.
3. If the B570 is busy or absent, the device is a config field.

Device selection passes through `GGML_VK_VISIBLE_DEVICES` on the spawned
process. The exact enumeration index must be confirmed at implementation time
against `llama-server --list-devices`, since Vulkan device order is not
guaranteed to match DRM card order.

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
llm::cleanup  (s1-mini)   ──► Note.body updated in place, state: Cleaned
      │
      ▼
llm::extract  (LFM2.5)    ──► Task rows created, state: Analyzed
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
swapping because both models together are under 3 GB and swapping would put a
multi-second stall in the fast path.

Startup sequence per model:

1. Probe `http://127.0.0.1:<port>/health`. If it answers, use it — this lets a
   user run their own server and lets Beamer survive its own restart without
   respawning.
2. If not, and `llm.manage_server = true`, spawn:
   `llama-server -m <gguf> --port <port> --host 127.0.0.1 -ngl 99 -c 4096`
   with `LD_LIBRARY_PATH` set to the llama.cpp build dir and
   `GGML_VK_VISIBLE_DEVICES` set to the configured device.
3. Poll `/health` until ready or a timeout (~30 s) elapses.

Servers are spawned **lazily on first use**, not at Beamer startup — no reason
to hold VRAM for a user who never dictates a note. An idle timer (default
15 minutes, configurable, 0 = never) shuts them down. Child processes are
killed on Beamer exit; a leaked `llama-server` holding VRAM would be a nasty
failure mode.

All process spawning and the blocking `/health` poll go through
`tokio::task::spawn_blocking`, per the threading rules in
`agent_docs/dioxus_architecture.md`.

### Request shape

`llama-server` exposes an OpenAI-compatible `/v1/chat/completions`. Beamer
already has `reqwest` and a shared `http_client()` in
`src/transcription/mod.rs` — the local calls reuse both, so this is the same
code shape as the existing cloud backends with a different base URL.

**Cleanup** (`llm/cleanup.rs`): system prompt instructs punctuation,
capitalization, disfluency removal, and inverse text normalization, with an
explicit instruction not to add, remove, or reword content. Response is plain
text. Temperature 0.

**Extraction** (`llm/extract.rs`): system prompt asks for a JSON array of
action items. Temperature 0. Request uses llama.cpp's
`response_format: {"type": "json_object"}` grammar constraint so the output is
structurally valid JSON by construction rather than by hope.

```json
{ "tasks": ["call the vet", "send Tuesday's invoice"] }
```

The parser must still tolerate ```json fences — models emit them regardless of
instructions, and the grammar constraint does not apply to every server version.

### Failure handling

The governing rule: **a failure never costs the user words.**

- Cleanup fails → `body` stays equal to `raw`, state becomes `CleanFailed`, the
  note shows a small retry affordance. The note is still perfectly usable.
- Extraction fails → state becomes `ExtractFailed`, no tasks created, retry
  available. The note is unaffected.
- Server unreachable, model file missing, GPU wedged, spawn failed → identical
  handling. Note creation itself never depends on any of it.

Errors are logged through the existing `StatusLog` so they surface in the app
rather than only in `RUST_LOG`.

### Trigger policy

- **Dictated notes:** cleanup runs automatically (you asked for correction, and
  you cannot proofread speech as you produce it). Extraction runs automatically
  after cleanup.
- **Typed notes:** neither runs automatically. Rewriting text someone
  deliberately typed is presumptuous. An explicit "Enhance" button on the note
  runs both.

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
vulkan_device = 0             # confirm against `llama-server --list-devices`
idle_shutdown_minutes = 15    # 0 = never

[llm.cleanup]
model_path = "…/s1-mini-Q8_0.gguf"
port = 8081

[llm.extract]
model_path = "…/LFM2.5-2.6B-Q6_K.gguf"
port = 8082

[notes]
all_workspaces = true         # Mutter: stick() notes across workspaces
default_color = "purple"
```

Model files are **not** downloaded by Beamer. The spec assumes they are fetched
manually (or by a small documented script) and their paths configured. Adding a
downloader is a separate feature with its own progress UI, error handling, and
disk-space concerns.

`agent_docs/config_schema.md` must be updated to match.

## 11. Testing

Unit-testable without a compositor or a GPU:

- **`notes::store`** — serialize/deserialize round-trip; corrupt-file preserved
  as `.corrupt` and a fresh store returned; debounce coalescing (many edits →
  one write); archive does not delete.
- **`llm` response parsing** — clean JSON; JSON wrapped in ```json fences;
  malformed JSON → error, not panic; empty response → error. Cleanup
  responses that come back empty must be rejected so `body` is never blanked.
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

## 13. Risks

- **`s1-mini` license is `license: other`.** It must be read before this
  ships. If it forbids the use, the fallback is to use LFM2.5 for both stages
  with a cleanup prompt — slower and less precise, but functional.
- **Vulkan device indices are not stable** across driver updates or card
  changes. `GGML_VK_VISIBLE_DEVICES=0` may not always mean the B570. The
  implementation should log the device llama-server actually selected at
  startup so a mismatch is visible rather than mysterious.
- **B60 wedge history.** Mitigated by defaulting to the B570, and by the rule
  that inference failure never blocks note creation.
- **Performance figures in §3 are estimates.** They must be benchmarked early;
  if stage 1 does not land under ~1 s, the "sticky updates while you watch"
  experience does not exist and the design of the fast path needs revisiting.
- **Two extra hotkey listeners** are the most likely source of platform-
  specific bugs, particularly on the evdev path.
- **Extension changes require a re-login** to test, which makes iteration on
  `PlaceWindow` slow. Budget for it.

## 14. Out of scope

Meeting/system-audio capture; local ASR; model downloading from within Beamer;
rich text; sync or sharing; recurring tasks, due dates or reminders; export to
external task managers; note templates.

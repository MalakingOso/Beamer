# Local inference — `src/llm/` and the note pipeline

Beamer runs one on-device model pass over a dictated note: **extraction**
proposes tasks. This document is about how it is wired and, mostly, about the
ways it fails *without saying so*, which is nearly all of them.

There used to be a second pass that rewrote the transcript before extraction
ran. It is gone, deleted across code, config, UI, preset and licence (see
`docs/decisions.md`, 2026-09-12). ElevenLabs already returns punctuated,
capitalized text, so the pass was normalizing words that needed no
normalizing, and every fresh install paid for the attempt with a red footer
label for a model its server was never told to serve. Extraction is the pass
that earns its place on a sticky.

Extraction re-reads `LlmConfig::extract_wanted()` from the live config
immediately before every store write, never from the snapshot `run_request`
took when the request started. A pass can be switched off while its request
is in the air. Writing anyway would record `Failed` on an error, or
suggestions on success, for a pass the user explicitly opted out of. An
abandoned pass reports `NotAttempted`, not `Errored`: nothing was learned
about the server, and an error would wrongly suppress the backlog sweep.

Read `agent_docs/sticky_notes.md` first for the note windows themselves.

## The shape of it

```
dictation ──> sink::do_note_capture ──> flush to disk ──> pipeline request
                                                              │
                          notes::pipeline (a coroutine in App())
                                              │
                                       llm::extract
                                  (K2-Horizon, json_object)
                                              │
                                    replace_suggestions
                                              │
                                          tasks.json
```

| Module | Job |
|---|---|
| `llm/mod.rs` | `LlmConfig`. Config only. |
| `llm/client.rs` | `GET /v1/models`. On-demand probe, nothing else. |
| `llm/chat.rs` | `POST /v1/chat/completions`, and the `ChatError` classification. |
| `llm/prompts.rs` | `extract_system(today)` (a policy). |
| `llm/extract.rs` | Fence stripping, evidence grounding, confidence floor. |
| `notes/pipeline.rs` | The coroutine that runs extraction and writes the results back. |
| `notes/lifecycle.rs` | The only code allowed to write the extraction result to a note. |
| `bin/task_eval.rs` | Measures extraction against the user's own accepted/dismissed rows. |

**Beamer never spawns the server.** It runs standalone and Beamer's entire
connection surface is the shared `[llm] base_url` plus an extract-only
override (`LlmConfig::extract_base_url()`, falling back to the shared value
when unset; see `agent_docs/config_schema.md`). On bearcave, extraction runs
locally, CPU-only, against `deploy/llama-models-bearcave.ini` (see
`agent_docs/running_on_bearcave.md`). Elsewhere it defaults to the same box
(`deploy/llama-beamer.service`).

`llama-server` has no authentication of its own, so it stays bound to
`127.0.0.1:8080` even when a client on another machine needs to reach it.
`tailscale serve --bg 8080` fronts that loopback port with a proxy on the
tailnet's own HTTPS certificate, so a client on the same tailnet can point
`base_url` at `https://<host>.<tailnet>.ts.net` with no code change: reqwest
is built with `native-tls`, and that validates against the OS trust store as
soon as HTTPS certificates are turned on for the tailnet in the admin
console. `--host 0.0.0.0` and Tailscale Funnel are both rejected on purpose:
the first puts an unauthenticated LLM API on every network the host joins,
the second is the same command pointed at the public internet.

## ⚠️ `src/llm/**` must contain no crate-rooted paths

There is no `src/lib.rs`, so `src/bin/task_eval.rs` reaches this code by
`#[path = "../llm/mod.rs"] mod llm;`. A single `use crate::…` anywhere under
`src/llm/` breaks that binary. This is self-policing — `cargo build` builds all
bins — but it is why `llm::extract::ProposedTask` exists separately from
`notes::task::Task` rather than the parser returning the store's type.

`src/notes/mod.rs` is **not** includable the same way: it reaches for
`crate::config::Config` to find its storage path. `task_eval` includes only the
leaf modules (`notes/model.rs`, `notes/task.rs` — both free of crate-rooted
paths) and reads the two JSON files with its own envelope structs.

The tests for `prompts.rs` and `extract.rs` live in `prompts/tests.rs` and
`extract/tests.rs`, reached by `#[path]` from their parents. **The same rule
applies inside them.**

## The failures that look like successes

Every item here returns HTTP 200 with a plausible body.

### 1. Thinking is on unless the preset turns it off — and K2-Horizon has no off

Every model here reasons by default.

- **K2-Horizon-0.9B** does not take a plain off switch. Its chat template reads
  `chat_template_kwargs.reasoning_effort` directly and opens the assistant turn
  with one of three tags — `high` (default) → `<ifm|think>`, `medium` →
  `<ifm|think_fast>`, `low` → `<ifm|think_faster>` — unconditionally. There is
  no fourth branch for "off": `enable_thinking: false` (the generic llama.cpp
  kill switch) is a **silent no-op** here, since the
  template never reads that name at all. `low` is what
  `deploy/llama-models-bearcave.ini` sets — the fastest of the three, not a
  disabled state.
- **Gemma 4** (pre-K2-Horizon default) filled `reasoning_content` and left
  `content` empty, or filled both and simply cost more.

Both switches live **server-side** in the models preset
(`deploy/llama-models.ini` on callisto, `deploy/llama-models-bearcave.ini` on
bearcave), and Beamer sends no sampling or template parameters of its own —
see below. That makes the preset the single owner, and a gap in it a gap
everywhere.

> **The Gemma version of this was a live bug until 2026-08-22.** Only the
> cleanup model had the setting; Gemma had been reasoning on every request
> since the preset was written. Nothing showed it — the status is 200, the JSON is valid, and
> the extracted tasks are byte-identical either way. Measured, same prompt and
> note: **306 predicted tokens / 4069 ms with thinking on, 64 tokens / 842 ms
> with it off.** Only the clock differs, which is why it survived so long.

`ChatError::ThinkingEnabled` exists for exactly this: empty `content` beside
non-empty `reasoning_content`. Without it the symptom arrives as an inscrutable
parse error.
It does **not** fire for K2-Horizon at any effort level: at `high` both
`content` and `reasoning_content` fill (so `content` is never empty); at
`medium`/`low`, `reasoning_content` is simply absent from the response, so
`reasoning` reads empty too and the check never trips either way — see the
next item.

### 1a. K2-Horizon's `reasoning_content` split only works at `high`

At `reasoning_effort: high`, llama-server's OAI-compatible response correctly
separates `reasoning_content` from `content`. At `medium`/`low`, it does not:
the reasoning trace, its closing tag, and the JSON answer all land
concatenated in `content`, e.g. `...Return JSON with tasks
array.\n</ifm|think_fast>\n{"tasks": [...]}\n`. The fork's fork-specific
chat-format parser (grep `common/chat.cpp` for `ifm`: zero matches) doesn't
generalize the `high`-tag split to the `_fast`/`_faster` variants.

`extract::strip_think_tags` (`src/llm/extract.rs`) fixes this client-side
rather than in the fork: it finds the **last** `</...think...>`-shaped closing
tag in the response body and takes everything after it, generically (any
`think`-containing closing tag, case-insensitive — also covers `<think>` for
free on a DeepSeek/Qwen-style server), and is wired into `parse_tasks` before
`strip_fences`. A tagless body passes through unchanged, so this costs nothing
against a server that never leaks the trace.

### 2. The request body carries the model and the messages and nothing else

No `temperature`, no `top_k`, no `chat_template_kwargs`, no `max_tokens`.
Re-sending them from Beamer would give one setting two owners and the
disagreement would be silent. `chat.rs`'s
`the_request_body_carries_no_sampling_parameters` pins their absence.

The one exception is `response_format: {"type":"json_object"}` on extraction,
which is a grammar constraint rather than a sampling parameter.

### 3. Never probe before a request

A read of `GET /v1/models` **resets the server's per-model idle clock**. A
health check on a timer therefore pins the ~3 GB extraction model in VRAM
permanently, with no error, no log line and no symptom until something else
needs the memory. Probe on button press and once when the settings page opens.
Nowhere else — and specifically not before a request that was going to be made
anyway. Connection-refused is fast and already well classified.

## Extraction is a precision problem

A fabricated task is worse than a missed one: a list nobody trusts cannot be
un-poisoned. Three mechanisms, in decreasing order of how much they matter.

**Accept/dismiss is the real one.** Nothing extraction produces is a fact. Rows
arrive as `Suggested`, surface as chips on the note, and one click decides
them. A false positive costs a click. Every decision is a labelled example, and
dismissed rows are **retained** — they are the negatives the eval corpus is
built from. Do not "clean them up".

**Evidence grounding is the structural one.** Every proposal carries the span of
the note that produced it, and `parse_tasks` drops any whose evidence is not in
the text that was actually sent. This is the only check that does not depend on
the model's judgment. Two details worth keeping:

- An **empty** evidence span is rejected explicitly. Every string contains the
  empty string, so a model that simply omitted the field would otherwise pass.
- Matching normalizes whitespace and case. A doubled space or a line break in a
  transcript is not a fabrication, and rejecting a genuine span over a capital
  letter would make the guard look broken while being useless.

**`min_confidence` is a backstop, not a mechanism.** Measured against the real
model, reported confidences cluster at **0.90–0.98** whatever the note, so the
0.5 default filters approximately nothing. Do not read "the floor dropped
nothing" as evidence that extraction is well calibrated, and do not reach for
the floor when precision needs work — the ladder and the prompt are the levers.

The floor is nonetheless applied **at parse time**, not at render time: a row
nobody is ever shown is not a labelled example, and letting it reach
`tasks.json` would record a decision nobody made.

## The prompt

`extract_system(today)` is tunable, but its *shape* is not. Its current shape
(`EXTRACT_BODY` in `src/llm/prompts.rs`) asks for a
`scan` array before `tasks`: one entry per clause in the note that names any
action, by anyone, each carrying `subject_is_speaker` (true only for the
note's own author) and `category` (one of eight, including `fact` — which
explicitly covers a recurring or ongoing problem, not just a one-off
statement). A clause only becomes a `tasks` entry when its own scan row says
`subject_is_speaker: true` **and** `category: "task"`. `TaskEnvelope` ignores
the extra `scan` field on deserialize, so nothing downstream needed to change
to carry it.

This shape exists because two failure modes did not respond to stronger
wording alone: a small model asked "is this concrete and dated" would say yes
to another person's dated promise, and telling it explicitly not to (an
earlier prompt version, tried and measured, not shipped) had no effect. Making
the subject check a separate, named field the model must fill in before
judging catches both a misattributed third-party commitment and a
recurring-problem statement dressed up as a commitment ("the server keeps
crashing" vs. "the server crashed once, I'll look into it").

Verbatim `evidence`/`due_phrase` (rejected downstream if not grounded — see
below), strict-or-null dates. Aspirations excluded deliberately: a poisoned
list cannot be un-poisoned.

Swept all 6 hand-written regression notes and most of a broader 14-note
edge-case set, against `K2-Horizon-0.9B-Q8_0` at `reasoning_effort: low`. Two
known open gaps, both still present after the `scan` structure landed:

1. **The model's own `scan` sometimes disagrees with its `tasks` output.** A
   clause scanned `subject_is_speaker: false, category: "someone_else"` still
   occasionally appears in `tasks` anyway — the judgment is right, the model
   just doesn't enforce it when assembling the final array. Not caught by any
   parse-time gate, because the clause text is a real, grounded span; this is
   a compliance gap, not a fabrication.
2. **Relative-date math beyond "tomorrow" is unreliable.** "Next Tuesday",
   "next weekend", "the 1st" sometimes resolve to the wrong day — including,
   at least once, a day in the *past* that still passes `resolve_date`'s
   one-day-of-slack gate because it happens to land within it. The gate
   catches an egregiously wrong date; it does not catch "right direction,
   wrong magnitude".

n=20 hand-checked notes is still a spot-check, not an evaluation. That is what
`task_eval` is for — see "Measuring it" below.

## Dated tasks

Extraction used to be **date-blind**: the model was handed the note and nothing
else, so "before Friday" was unresolvable in principle, not by accident.

`prompts::extract_system(today)` now states the day — **by name as well as by
number** ("Sunday, 23 August 2026 (2026-08-23)"), because asking a language
model to compute a weekday from an ISO date is asking it to be wrong.

⚠️ **The date goes in the *system* message.** The note is still sent byte for
byte as the user message, and a test pins that.

Three new fields per proposal:

| Field | Meaning |
|---|---|
| `due` | ISO date or datetime, **or null**. Only when concretely resolvable. |
| `due_phrase` | The **verbatim** span the date was read from. |
| `kind` | `"todo"` (a deadline) or `"event"` (an appointment at a stated time). |

The prompt names the vague phrasings — "sometime next week", "soon", "in a bit"
— and requires `due: null` for them **while still returning the phrase**. That
asymmetry is the design: a model asked for a date will produce one, so the
policy is strict; and showing the user the words the model saw, beside a date
picker, turns a dead end into one click.

### The four gates, in `extract::resolve_date`

Every one **downgrades rather than discards**. A date the model got wrong costs
the date, never the task — losing a real commitment because its date failed to
parse would be the worst trade available.

1. **`due_phrase` must ground in the note**, via the same normalize-and-contains
   check `evidence` gets. The strongest guard available against an invented
   date, and it costs nothing new: a model that made the date up made the phrase
   up too. Fails → both fields dropped.
2. **`due` must parse.** Unparseable → keep the phrase, drop the date.
3. **`due` may not be more than a day in the past.** "Friday" resolved against
   the wrong year is the classic failure and is otherwise completely silent —
   the task looks perfect and is filed under 2024. One day of slack, not zero,
   so a pass running just after midnight is not thrown away.
4. **An unrecognised `kind` becomes `Todo`.**

The model's own `due_all_day` flag is deliberately **not** read: whether a value
names a day or a moment is decided by the shape of `due` itself, because the two
can contradict each other and the value is the half carrying the information.

`extract::extract` takes `today` as a **parameter**, not a clock read, so the
gates are testable without mocking time — and so `task_eval` can grade each note
against the day it was *captured*. Grading "before Friday" against the day the
eval happens to run measures nothing.

Nothing reaches a calendar automatically. `notes/ics.rs` emits a `VTODO` or
`VEVENT` on an explicit per-task click, which follows directly from tasks being
suggestions. Two traps live there, both tested: DATE-TIME has exactly three
valid forms and **an offset suffix is not one of them** (timed values are
emitted floating), and folding is defined in octets but must break on character
boundaries.

⚠️ **Date extraction quality is unmeasured**, exactly like the extraction prompt
it extends. It is contained by design rather than by hope — strict resolution,
the grounding gate, and a click before anything leaves the app. Extend
`task_eval` to grade dates once a few dozen dated notes exist.

## Trigger policy

- **Dictated notes** analyse automatically. You cannot proofread speech as you
  produce it, and a task nobody proposes is a task nobody does.
- **Typed notes** do not analyse unasked. Proposing tasks for text somebody
  deliberately typed is a guess about intent nobody asked for.

This is **structural, not a runtime check**, for the *first* automatic pass on
a fresh note. That decision lives at exactly one site, `sink::do_note_capture`,
which is reachable only from dictation. A typed note has no path to that line.
Keep it that way rather than adding an `if origin == Dictated` somewhere: the
check would be forgettable and the topology is not.

Extraction can be re-run from the note's footer, which reads `extract_state`
rather than the note's origin. Keying it to origin leaves a dead end: a
dictated note that already ran its automatic pass has spent its trigger and
would have no way back.

⚠️ **A second, different kind of automatic trigger exists: the backlog sweep**
(`pipeline::sweep_requests`, described in full under "Failure handling"
below). It is not gated by origin at all, and that is deliberate rather than
an oversight. `sink::do_note_capture` decides *whether a note gets analysed
in the first place*, and that decision does stay origin-gated exactly
as described above. The sweep does something narrower: it re-sends a request
for extraction that already reached `Failed`, once a later success proves the
server is reachable again. A typed note that reached `Failed` by way of a
footer press is swept the same as a dictated one. The user already asked once,
by pressing retry; the sweep is only carrying that same ask forward, not
inventing a new automatic pass on text nobody asked to have touched. If that
distinction ever stops holding, i.e. if the sweep starts running passes a note
never had a human ask for, the origin gate belongs on `sweep_requests` too.

## The pipeline coroutine

**It lives in `App()`, not in the window that asked for it.** Dioxus drops a
spawned task when its owning scope drops (`dioxus-core-0.7.9/src/tasks.rs:159`),
so a pass started from a sticky's scope would be **silently cancelled** by
closing that note mid-flight. `App()`'s scope outlives every note window.

⚠️ Dioxus `spawn`, never `tokio::spawn` — desktop's tokio runtime is
multi-threaded and `Signal`'s generational-box arena is thread-local.

In-flight requests are driven together by a `FuturesUnordered` **inside the
coroutine's single future**, not by spawning a task each. A serial loop would
let one hung request stall every later note for the full `request_timeout_ms`;
spawning per request would put scope ownership back in question for no gain.
Duplicate requests for a note already in flight are dropped: two passes would
race on the same suggestions and the loser's work would be discarded.

Extraction needs no compare-and-swap guard: chips carry their own evidence, so
a stale suggestion is visibly stale rather than silently wrong.

## Failure handling

The governing rule: **a failure never costs the user words.** The note is
created and flushed to disk *before* any model is contacted, so a server that
is down, a model file that is missing or a busy GPU all degrade to "the note
has no suggestions yet", never to a lost note.

| Outcome | `extract_state` | Effect |
|---|---|---|
| Suggestions proposed | `Done` | chips updated |
| No tasks found | `Done` | nothing changes |
| Network, non-2xx, timeout | `Failed` | footer shows a red label |
| `llm.enabled = false` | `Skipped` | only if the stage had never run |
| Backlog sweep, after a later success | `Failed` -> retried | see below |

`Skipped` never overwrites `Done`, so asking for a pass while the feature is
off cannot erase the record that it once ran.

### The backlog sweep

Before the sweep, a `Failed` note just sat there until the user noticed the
red label and pressed the footer. That is fine for a single note failing once,
and wrong for a server that was briefly unreachable during a run of several
notes: nobody wants to click retry five times because their machine happened
to be busy when they first dictated.

`use_pipeline` (`src/notes/pipeline.rs`) now sweeps the backlog itself: when a
pass finishes and succeeded, it also re-sends a `PipelineRequest` for every
non-archived note whose `extract_state` is `Failed` (`sweep_requests`). This
includes the note that just finished: `in_flight` removal (`pipeline.rs:84`)
runs before the sweep (`pipeline.rs:96`), so a retry that succeeds still
sweeps that same note if its extraction is somehow still `Failed`. Archived
notes are excluded on purpose: archiving is the user saying they are done with
a note, and a `Failed` stage on one is not backlog to keep spending requests
on. The existing `in_flight` set still dedupes, so this cannot storm the
server with duplicate requests, and a swept request is marked `swept: true` so
*its own* completion never triggers a further sweep. Without that guard, a
note that keeps genuinely failing would re-sweep the whole backlog forever,
once per success, on every failed note in the app.

**"Succeeded" means a response actually arrived.** A pass can finish
having contacted the server zero times: the pass was disabled, the whole
feature was disabled, the note vanished before a request could go out, or
there was nothing to send (a blank note). None of those prove the server is
up, so none of them count. `succeeded` is a direct comparison against
`RequestOutcome::Responded` (`src/notes/pipeline/sweep.rs`): with one pass,
no fold is left to write.

**This does not violate the never-poll rule.** The rule is about a timer: a
periodic `GET /v1/models` that runs whether or not anyone asked for anything,
which resets the server's per-model idle clock and pins a model in VRAM with
no error and no symptom. The sweep has no timer and starts nothing on its
own. It only ever fires as a direct, synchronous consequence of a request
that was already going to happen, already succeeded, and already proved the
server is reachable right now. No new request is sent unless a person's own
action (dictating, pressing the footer) produced one first.

Errors surface through `StatusLog` as well as `RUST_LOG`.

## Models on disk

`K2-Horizon-0.9B` is a third-party GGUF quant
(`NANI-Nithin/K2-Horizon-0.9B-GGUF`, Q8_0) of IFM's BF16 release: llama.cpp
upstream cannot even load it (see "The standalone server's own build" below),
so there is no publisher GGUF to prefer. No `mmproj` (vision) file is needed,
as Beamer's use is text-only.

| File | Size | Role | Where it lives |
|---|---|---|---|
| `K2-Horizon-0.9B-Q8_0.gguf` | 1.15 GiB | Task extraction. | bearcave (`%USERPROFILE%\models\beamer\`) |

Licences are in `licenses/`. K2-Horizon's licence terms are **unresolved as of
this writing** — see the provenance note in `licenses/K2-Horizon-LICENSE.txt`.

**Gemma 4 (the previous extraction model) is retired from bearcave's default,
but still what callisto-hosted Beamer instances should use**, since upstream
llama.cpp — which callisto's `deploy/llama-beamer.service` builds — has no
`K2HorizonForCausalLM` support at all. Its old model ladder:

| Rung | Model | Disk | Note |
|---|---|---|---|
| 0 | `google/gemma-4-E2B-it-qat-q4_0-gguf` | 3.12 GiB | Smaller/faster; downgrade option if latency binds |
| 1 | `google/gemma-4-E4B-it-qat-q4_0-gguf` | 4.80 GiB | Former default |
| 2 | `unsloth/gemma-4-12B-it-qat-GGUF` (UD-Q4_K_XL) | 6.72 GB | |
| 3 | `google/gemma-4-26B-A4B-it-qat-q4_0-gguf` | 13.45 GiB | **The slowest measured**, 43.6 t/s — see below |

⚠️ **Corrected on measurement.** An earlier assumption held that rung 3 might
be the fastest per token, reasoning from bytes read per token. Measured on the
B60 it is the **slowest**: 43.6 t/s against E4B's 77.3 and E2B's 116.0. The
read-per-token argument assumes bandwidth-bound decoding, and expert routing
and gather overhead dominate here. On this hardware the ladder is monotonic in
size — **smaller is faster** — so rung 3 is a *quality* option only, never a
speed play.

K2-Horizon has no equivalent ladder yet: only the 0.9B size exists in the
K2-Horizon family as of this writing, and the choice actually exercised was
across **quant and reasoning effort**, not size — see "Rejected on
measurement" below.

## The standalone server's own build

### bearcave's extraction server — a different fork, a different build

K2-Horizon's architecture (`K2HorizonForCausalLM`) is not in upstream
llama.cpp at all — the model fails to load, full stop. Bearcave's extraction
server is therefore built from **`MBZUAI-IFM/llama.cpp`, branch
`model/K2Horizon`**, not the upstream tree callisto's server uses.
That fork also needed a tokenizer patch: K2-Horizon's pretokenizer regex
carries a `\uXXXX` escape that MSVC's `std::regex`/`std::wregex` cannot parse,
so `unicode_regex_split_custom_k2_horizon()` was added to `src/unicode.cpp`
(copied from `unicode_regex_split_custom_llama3`'s shape, extended for
`\p{M}` combining marks and literal ZWJ/ZWNJ codepoints).

Registered as the Scheduled Task **"Beamer K2-Horizon Server"** (runs at
logon), not a systemd unit — bearcave is Windows. See
`agent_docs/running_on_bearcave.md` for the full setup and
`deploy/llama-models-bearcave.ini` for the preset. Runtime is 9 files (the
exe, its 4 direct DLL deps, 3 `ggml*.dll`, and `libomp140.aarch64.dll` — the
release OpenMP runtime; only the debug variant ships in `System32`, so it
must be copied in beside the exe or the server fails to start with no error
text), not the full ~736 MB dev checkout. CPU-only by design: bearcave has no
GPU, which is the entire point of choosing a 0.9B model here.

#### Getting the runtime + model onto a fresh install

A locally-built NSIS installer (`installer/k2horizon/hooks.nsh`) now does
this automatically for a bundled aarch64 build — see
`agent_docs/running_on_bearcave.md`'s "Local extraction" section for the
full walkthrough. Three things worth knowing if touching this code:

- **The runtime and the model live at a fixed per-user path**
  (`%LOCALAPPDATA%\Beamer\llama-k2horizon\`,
  `%USERPROFILE%\models\beamer\...`), independent of whether Beamer itself
  was installed per-user or per-machine. Beamer runs `asInvoker` and a
  per-machine install puts the app under `Program Files`, which an
  unelevated process can't write into later — so neither the runtime nor the
  (much larger, downloaded-not-bundled) model can live inside the app's own
  install directory.
- **`src/model_setup.rs` starting the Scheduled Task is the one narrow,
  deliberate exception to "Beamer never touches server lifecycle."** It
  shells out to `schtasks /run` exactly once, after downloading and
  sha256-verifying the model, to start a task the installer registered
  *dormant*. It never spawns `llama-server.exe` directly, and nothing else
  in the app ever calls into Task Scheduler.
- **`[bundle].resources` does not work for bundling arbitrary files into an
  NSIS payload in this `dioxus-cli` version** — tried first, disproven
  empirically (nothing referencing a `resources` glob entry ever appeared in
  the generated `.nsi` or its staging directory; only manganis `asset!()`
  output does). `hooks.nsh` instead embeds the runtime files directly via
  NSIS's own `File /nonfatal "<path>\*.*"`, which also downgrades a
  zero-match glob (the CI scenario — `vendor/llama-k2horizon/` is gitignored
  and never populated there) from a compile error to a harmless warning,
  verified both ways.

### callisto's server

The server is `deploy/llama-beamer.service` on callisto, not spawned by
Beamer. Backend is **SYCL, not Vulkan** — measured **2.35x** Vulkan at prompt
processing, **~1.2x** at generation, same commit and device. Build dirs:

- `build-sycl-2026/` — **what the service runs.** oneAPI 2026.1,
  `GGML_SYCL=ON`, `GGML_SYCL_F16=ON`, icx/icpx.
- `build-sycl/` — same flags on oneAPI 2025.3, kept as fallback. Measured
  identical to the 2026 build on gemma-4-E4B (pp512 2781.64 vs 2782.35 t/s,
  tg128 79.80 vs 79.83) across eight paired runs, so the newer toolchain is
  housekeeping, not speed.
- `build/` — Vulkan fallback, no oneAPI runtime needed.

Two more traps beyond the four already covered above, both about the server
process rather than the model:

- ⚠️ **`-dev SYCL1`, not an env mask.** `ZE_AFFINITY_MASK` /
  `GGML_VK_VISIBLE_DEVICES` *filter* the device list, so the B60 re-indexes to
  0 and the mask value stops matching the in-process id. `-dev` selects from
  the full list. The B60 is index **1** on both backends — verified, not
  assumed.
- ⚠️ **`LD_LIBRARY_PATH` must extend oneAPI's, not replace it.** Setting it
  outright drops `libsvml.so` and the server dies naming a library nothing
  else mentions.

Both are already applied in `deploy/llama-beamer.service`; this is the "why",
not a config that still needs doing.

## Measuring it

```bash
cargo run --bin task_eval -- --limit 20
cargo run --bin task_eval -- --model K2-Horizon-0.9B-Q8_0 --base-url http://127.0.0.1:8080
```

Run against bearcave's real corpus (2026-09-04, 11 of 33 notes carried
decided rows; the rest were skipped for want of labels): **62.5% precision (5
TP / 8 judged), 50.0% recall (5 TP / 10 accepted rows), 0 errors, mean 7.98
s/note.** Several of the "missed" rows were near-misses on trailing
punctuation in `evidence` rather than a genuinely absent task — `match_row`
matches text and evidence by normalized equality, and normalization collapses
whitespace and case but not punctuation, so a proposal missing a note's
trailing period does not match a decided row that has one. Worth relabeling
those specific notes in the app before trusting the recall number as final;
this is a matcher-strictness effect, not necessarily a K2-Horizon-specific
one. Read the FP/FN rows the tool prints before drawing a conclusion — several
of the FPs on this run look like reasonable extractions the user dismissed
for reasons outside the note's text (already handled, or a throwaway test
note), not model misjudgment.

The corpus is the user's own notes plus their accept/dismiss decisions; a note
with no decided rows carries no labels and is skipped rather than guessed at.

⚠️ **A proposal matching neither an accepted nor a dismissed row is reported
UNGRADED, never scored as a false positive.** It may be a genuinely new and
correct suggestion the user never saw. Scoring it wrong would make the harness
lie in the direction that looks like rigour.

## Measured, so stop estimating

Callisto figures: Arc Pro B60 via SYCL, `-dev SYCL1`, thinking off, 2026-08-22
unless noted. Bearcave figures: Snapdragon X (aarch64), CPU-only, no GPU
backend, `reasoning_effort: low` unless noted.

| Thing | Measured |
|---|---|
| Gemma E4B extraction, thinking **off** (callisto) | **0.13–3.0 s**, typically ~1.1 s |
| Gemma E4B extraction, thinking **on** (callisto) | 4.1 s for identical output |
| Wake Gemma from sleep (callisto) | 1.68 s |
| Gemma cold start, spawn + 4.8 GB load (callisto) | 4.06 s |
| Resident VRAM, both callisto models | ~4.3 GB of 22.7 GB |
| K2-Horizon-0.9B-Q8_0 extraction, `low`, 14-note batch (bearcave) | 1.3–12.4 s/note |
| K2-Horizon-0.9B-Q8_0 extraction, `low`, real-corpus `task_eval` run (bearcave) | mean 7.98 s/note, 0 timeouts against a 60 s budget |
| K2-Horizon-0.9B-Q8_0 extraction, `medium` (bearcave) | ~2–85 s/note, 100–1430 tokens |
| K2-Horizon-0.9B-Q8_0 extraction, `high` (bearcave) | 5–95 s/note typical; **hung past 120 s on one note** in testing |
| K2-Horizon-0.9B-Q8_0 resident RAM (bearcave) | ~1.15 GB — no reason to sleep it |

The Gemma ladder was **monotonic in size on the B60: smaller is faster.**
Rung 3 (`gemma-4-26B-A4B`) was the slowest measured, not the fastest — the
bytes-read-per-token argument assumes bandwidth-bound decoding and expert
routing dominates there. It was a *quality* option only. K2-Horizon has not
been measured against a size ladder (see "Models on disk" above);
the axis that mattered for it was quant and reasoning effort instead (below).

## Rejected on measurement — do not re-propose without new evidence

- **n-gram speculative decoding** (`--spec-default`, callisto/Gemma). The
  apparent 5x was an artifact of re-running identical text against cached
  output. On unseen notes it does not engage; tuned shorter it accepts ~30%
  and nets a wash.
- **Ollama.** No Intel Arc support in the standard release; Intel's path is
  the retired IPEX-LLM fork.
- **K2-Horizon at `reasoning_effort: medium` or `high`, despite each fixing a
  real gap `low` has.** `medium`/`high` correctly classify a recurring-problem
  note that `low` hallucinates a task from — genuine, reproduced information.
  But neither is a strict upgrade: `medium` then drops a different, correctly-
  extracted task elsewhere; `high` hung past 120 s (retried at 240 s, still no
  response) on one note in a 6-note test set. An extraction pass meant to run
  automatically after every dictated note cannot carry an unbounded tail, so
  `low` ships despite its own known miss.
- **K2-Horizon-0.9B-Q6_K, to recover `medium`/`high`'s fix at less cost than
  Q8_0.** Genuinely 30–50% faster than Q8_0 at matched effort and does carry
  the same fix. But across the same test matrix it also produced two failures
  Q8_0 never did: a non-terminating repetition loop on one note, and a
  complete miss (zero tasks) on another — a different, worse class of problem
  than a wrong judgment call. Q8_0 remains the quant used.

## Licence note

⚠️ **K2-Horizon-0.9B's licence terms are unresolved, not confirmed-permissive.**
`IFM/K2-Horizon-0.9B`'s own metadata pairs an `apache-2.0` SPDX tag with
`license_name: internal-only` and a `license_link` pointing at a `LICENSE`
file that does not exist in the repository (checked 2026-09-04). That
combination is the same shape that has signaled a binding additional term
before, not a plain-Apache repo. The GGUF quant used here
(`NANI-Nithin/K2-Horizon-0.9B-GGUF`) just defers back to the same missing
file. `licenses/K2-Horizon-LICENSE.txt` carries the canonical Apache 2.0 text
as the best available floor and a full provenance note — re-check the source
repository for a published `LICENSE` file before treating this as settled.

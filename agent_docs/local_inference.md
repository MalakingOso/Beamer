# Local inference — `src/llm/` and the note pipeline

Beamer runs two on-device model passes over a dictated note: **cleanup**
rewrites the transcript, **extraction** proposes tasks. This document is about
how they are wired and, mostly, about the ways they fail *without saying so* —
which is nearly all of them.

Read `agent_docs/sticky_notes.md` first for the note windows themselves.

## The shape of it

```
dictation ──> sink::do_note_capture ──> flush to disk ──> pipeline request
                                                              │
                          notes::pipeline (a coroutine in App())
                                    │
                    ┌───────────────┴───────────────┐
              llm::cleanup                     llm::extract
              (s1-mini, plain text)            (gemma, json_object)
                    │                                │
              apply_cleanup (CAS)            replace_suggestions
                    │                                │
                notes.json                       tasks.json
```

| Module | Job |
|---|---|
| `llm/mod.rs` | `LlmConfig`, `MODEL_CREDIT`. Config only. |
| `llm/client.rs` | `GET /v1/models`. On-demand probe, nothing else. |
| `llm/chat.rs` | `POST /v1/chat/completions`, and the `ChatError` classification. |
| `llm/prompts.rs` | `CLEANUP_SYSTEM` (a wire format) and `extract_system(today)` (a policy). |
| `llm/cleanup.rs` | Stage 1. Builds the request; decides what a response *means*. |
| `llm/extract.rs` | Stage 2. Fence stripping, evidence grounding, confidence floor. |
| `notes/blocks.rs` | The placeholder-token grammar. Pure. **Everything below depends on it.** |
| `notes/pipeline.rs` | The coroutine that runs both, segments the body, and writes the results back. |
| `notes/lifecycle.rs` | The only code allowed to write a stage result to a note. |
| `bin/task_eval.rs` | Measures extraction against the user's own accepted/dismissed rows. |

**Beamer never spawns the server.** It runs standalone (`deploy/llama-beamer.service`)
and Beamer's entire connection surface is `base_url`. See `agent_docs/config_schema.md`.

## ⚠️ `src/llm/**` must contain no crate-rooted paths

There is no `src/lib.rs`, so `src/bin/task_eval.rs` reaches this code by
`#[path = "../llm/mod.rs"] mod llm;`. A single `use crate::…` anywhere under
`src/llm/` breaks that binary. This is self-policing — `cargo build` builds all
bins — but it is why `llm::extract::ProposedTask` exists separately from
`notes::task::Task` rather than the parser returning the store's type.

`src/notes/mod.rs` is **not** includable the same way: it reaches for
`crate::config::Config` to find its storage path. `task_eval` includes only the
leaf modules (`notes/model.rs`, `notes/task.rs`, `notes/blocks.rs` — all three
free of crate-rooted paths) and reads the two JSON files with its own envelope
structs.

The tests for `prompts.rs` and `extract.rs` live in `prompts/tests.rs` and
`extract/tests.rs`, reached by `#[path]` from their parents. **The same rule
applies inside them.**

## The failures that look like successes

Every item here returns HTTP 200 with a plausible body.

### 1. Thinking is on unless the preset turns it off

Both models reason by default and neither was trained to.

- **S1-mini** inherits Qwen3's template. With thinking on it emits `<think>`
  and stops after about three tokens. Do **not** substitute `reasoning-budget 0`
  — the model card says output degrades. It also needs greedy decoding forced,
  because the GGUF carries `temp 0.6 / top_p 0.95 / top_k 20` inherited from
  Qwen3-0.6B.
- **Gemma 4** fills `reasoning_content` and leaves `content` empty, or fills
  both and simply costs more.

Both switches live **server-side** in `deploy/llama-models.ini`, and Beamer
sends no sampling or template parameters of its own — see below. That makes the
preset the single owner, and a gap in it a gap everywhere.

> **This was a live bug until 2026-08-22.** Only s1-mini had the setting;
> Gemma had been reasoning on every request since the preset was written.
> Nothing showed it — the status is 200, the JSON is valid, and the extracted
> tasks are byte-identical either way. Measured, same prompt and note:
> **306 predicted tokens / 4069 ms with thinking on, 64 tokens / 842 ms with it
> off.** Only the clock differs, which is why it survived so long.

`ChatError::ThinkingEnabled` exists for exactly this: empty `content` beside
non-empty `reasoning_content`. Without it the symptom arrives as an inscrutable
parse error, or — worse for cleanup — as "the model cleaned it to nothing".

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

### 4. An empty cleanup response is success

Say "um, uh, so" into a note and S1-mini correctly returns an **empty string**;
there was nothing to normalize. Treating that as failure is merely wrong;
treating it as a rewrite is destructive, because it blanks the only record of
what was said. `body` stays equal to `raw` and `clean_state` becomes `Done`.

Pinned at both layers — `cleanup::resolve` and `lifecycle::apply_cleanup` —
because the intuitive implementation gets it backwards.

### 5. S1-mini's input is a wire format, not a prompt

The publisher documents that a reworded system prompt or an out-of-set control
value produces garbled output. So `prompts::control_line` takes **only enums**:
there is no `&str` path by which an untrained value could reach the model, and
an unrecognised config value falls back to the trained default rather than
being passed through.

Trained sets: `Styling` ∈ {casual, semi-casual, semi-formal, formal},
`Structure` ∈ {prose, lists}, `Context` ∈ {general, email}.

> **`structure = "lists"` rarely produces lists.** Measured on a filler-heavy
> three-item note, S1-mini returned prose: *"So I need to call the vet about
> Milo, and also send Tuesday's invoice, and I guess pick up the dry cleaning
> at some point."* The model is conservative about bullets. This is why
> markdown rendering inside the note's `<textarea>` is a non-issue in practice
> rather than a deferred problem — list markers are rare, not merely unstyled.

### 6. A placeholder token in the body is out-of-distribution input

A note holding an image carries a `[[beamer:<id>]]` line in its `body` (see
`agent_docs/sticky_notes.md`). `body` is what cleanup is handed and what it
overwrites wholesale, and s1-mini has never seen a token like that. The result
is failure mode 5 by another route: garbled text, HTTP 200, plausible body.

**So a token is never sent. Not escaped, not quoted — removed.**

```
body ──blocks::parse──> [Text a][Attach x][Text b]
                           │                  │
                      clean(a)            clean(b)     <- two calls, no tokens
                           └── blocks::reassemble ──┐
                                                    ▼
        apply_cleanup(id, expected = the ORIGINAL FULL body, reassembled)
```

Six rules, in `pipeline::run_cleanup`. Each one preserves an existing behaviour
rather than adding one:

1. `sent` is still the **whole** body, so the compare-and-swap is unchanged and
   an edit mid-pass still supersedes.
2. Each `Block::Text` run is cleaned independently by the existing
   `cleanup::clean`. Runs go over **verbatim** — `cleanup_user_message` is not
   touched, so the control line is still the wire format it always was.
3. A blank run is not sent at all and passes through.
4. A run answering `NothingToChange`, or with an empty reply, keeps its
   original text. `reassemble` cannot make a run empty, which is what keeps
   `apply_cleanup`'s `!cleaned.trim().is_empty()` guard meaningful.
5. If every run had nothing to change, the pass reports that and the body is
   not rewritten.
6. **A note with no attachments yields exactly one run** — one call carrying
   the whole body, today's behaviour, reproduced *by construction*. There is no
   fast-path flag that could get out of step.

An HTTP error on any run **aborts the pass**: `mark_clean_failed`, nothing
applied. Half a cleaned note is worse than an uncleaned one, and the footer's
retry re-runs the whole thing.

Cost is one call per run — 0.225 s each measured, so a note with two images is
about 0.7 s. Serial on purpose: concurrency here buys a fraction of a second
and risks reordering the answers.

**Extraction** gets `blocks::plain_text(&body)` — the same tokens removed. That
also means an `evidence` span can never contain token text, because
`is_grounded` checks against the string that was actually sent.

The invariant is pinned at the pure layer, in `blocks`'s tests: a two-image body
yields three text runs and **no run contains `[[beamer:`**. `cleanup::clean` is
HTTP, so the call count itself is not unit-testable — verify it in
`RUST_LOG=beamer=debug` instead, where no request body may contain a token.

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

`extract_system(today)` is tunable — unlike `CLEANUP_SYSTEM` — but its *shape*
is not.
It enumerates seven negative categories with examples, states that most notes
contain no tasks and that empty is the correct and common answer, and carries
**two hard-negative exemplars**. Positive-only exemplars teach a model that
output is always expected, which is the same over-triggering failure the
categories exist to suppress. Aspirations are excluded deliberately: they are
the largest ambiguous class.

Spot-checked against the live model on 2026-08-22 (E4B, thinking off):

| Probe | Result |
|---|---|
| One commitment beside two planted negatives | exactly one task |
| Pure venting | `{"tasks": []}` |
| Past actions plus a fact | `{"tasks": []}` |
| Aspirations phrased like commitments | `{"tasks": []}` |
| Three real commitments | all three, all grounded |

n=5 is a spot-check, not an evaluation. That is what `task_eval` is for.

## Dated tasks

Extraction used to be **date-blind**: the model was handed the note and nothing
else, so "before Friday" was unresolvable in principle, not by accident.

`prompts::extract_system(today)` now states the day — **by name as well as by
number** ("Sunday, 23 August 2026 (2026-08-23)"), because asking a language
model to compute a weekday from an ISO date is asking it to be wrong.

⚠️ **The date goes in the *system* message.** The note is still sent byte for
byte as the user message, and a test pins that. ⚠️ **Safe here and only here** —
Gemma is a general instruct model. The same move against s1-mini is failure
mode 5; cleanup's prompt is not touched.

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

- **Dictated notes** clean and analyse automatically. You asked for correction,
  and you cannot proofread speech as you produce it.
- **Typed notes** do neither unasked. Rewriting text somebody deliberately typed
  is presumptuous, and S1-mini is a *transcript* normalizer — typed prose is
  outside its training distribution.

This is **structural, not a runtime check**. The automatic trigger lives at
exactly one site, `sink::do_note_capture`, which is reachable only from
dictation. A typed note has no path to that line. Keep it that way rather than
adding an `if origin == Dictated` somewhere — the check would be forgettable
and the topology is not.

Either pass can be re-run from the note's footer, which reads the two stage
fields rather than the note's origin. Keying it to origin leaves a dead end: a
dictated note whose cleanup was superseded by an edit has spent its automatic
trigger and would have no way back.

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
Duplicate requests for a note already in flight are dropped — two passes would
race on the same compare-and-swap and the loser's work would be discarded.

### The compare-and-swap

`sticky.rs` writes `body` on **every keystroke**. Cleanup therefore captures the
body at send time and `lifecycle::apply_cleanup` applies the result only if the
body still equals it. Otherwise the user typed while the model was thinking,
their edit wins, and the pass returns `Superseded` having changed *nothing* —
not even `clean_state`, so the footer still offers a retry.

⚠️ The guard is **body equality, not a `modified` timestamp**. `set_color` and
`set_open` bump `modified` for things that are not edits, so a timestamp guard
would reject perfectly valid results.

Extraction needs no such guard: chips carry their own evidence, so a stale
suggestion is visibly stale rather than silently wrong.

## Failure handling

The governing rule: **a failure never costs the user words.** The note is
created and flushed to disk *before* any model is contacted, so a server that
is down, a model file that is missing or a busy GPU all degrade to "the note is
not cleaned yet", never to a lost note.

| Outcome | `clean_state` | Effect |
|---|---|---|
| Rewritten | `Done` | `body` replaced, `raw` untouched |
| Empty / unchanged response | `Done` | nothing changes |
| Superseded by an edit | `Pending` | nothing changes; footer still offers it |
| Network, non-2xx, timeout | `Failed` | `body` stays; footer shows a red label |
| `llm.enabled = false` | `Skipped` | only if the stage had never run |

Extraction is independent: a failed cleanup still runs extraction, against
`body` — which equals `raw` when cleanup failed, and equals the user's own text
when it was superseded. `Skipped` never overwrites `Done`, so asking for a pass
while the feature is off cannot erase the record that it once ran.

Errors surface through `StatusLog` as well as `RUST_LOG`.

## Measuring it

```bash
cargo run --bin task_eval -- --limit 20
cargo run --bin task_eval -- --model gemma-4-E2B_q4_0-it   # walk the ladder
```

The corpus is the user's own notes plus their accept/dismiss decisions; a note
with no decided rows carries no labels and is skipped rather than guessed at.

⚠️ **A proposal matching neither an accepted nor a dismissed row is reported
UNGRADED, never scored as a false positive.** It may be a genuinely new and
correct suggestion the user never saw. Scoring it wrong would make the harness
lie in the direction that looks like rigour.

## Measured, so stop estimating

Arc Pro B60 via SYCL, `-dev SYCL1`, thinking off, 2026-08-22 unless noted.

| Thing | Measured |
|---|---|
| S1-mini cleans a filler-heavy ~25-word note | **109 ms** |
| S1-mini cleans a ~60-word note | 225 ms |
| Gemma E4B extraction, thinking **off** | **0.13–3.0 s**, typically ~1.1 s |
| Gemma E4B extraction, thinking **on** | 4.1 s for identical output |
| Wake extraction model from sleep | 1.68 s |
| Cold start (spawn + 4.8 GB load) | 4.06 s |
| Resident VRAM, both models | ~4.3 GB of 22.7 GB |

The model ladder is **monotonic in size on this hardware: smaller is faster.**
Rung 3 (`gemma-4-26B-A4B`) is the slowest measured, not the fastest — the
bytes-read-per-token argument assumes bandwidth-bound decoding and expert
routing dominates here. It is a *quality* option only.

## Rejected on measurement — do not re-propose without new evidence

- **n-gram speculative decoding** (`--spec-default`). The apparent 5x was an
  artifact of re-running identical text against cached output. On unseen notes
  it does not engage; tuned shorter it accepts ~30% and nets a wash.
- **Ollama.** No Intel Arc support in the standard release; Intel's path is the
  retired IPEX-LLM fork.

## Licence obligation

`MODEL_CREDIT` in `src/llm/mod.rs` is not a courtesy. `superwhisper/s1-mini` is
Apache 2.0 **plus a binding additional term** requiring the model to be
identified as `"S1-mini" by "Superwhisper"` — that exact capitalization. It is
pinned by an exact-equality test whose comment explains that the risk is not
malice but tidiness.

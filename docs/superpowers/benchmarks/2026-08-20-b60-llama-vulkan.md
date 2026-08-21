# B60 llama.cpp benchmark — Task 1

> **Status: COMPLETE**, except page-cache-cold start, which needs a `sudo`
> the agent cannot run. Every threshold in the plan now has a measured answer.

**Date:** 2026-08-21
**Machine:** berkley@CAllisto — Ryzen 9 7950X3D, Intel Arc B570 + Arc Pro B60
**Goal:** replace the sticky-notes spec's estimated latency figures with measurements.

## Environment

| Item | Value |
|---|---|
| llama.cpp commit | `5a32f7b66` (2026-08-21), up from `e97492369` (2026-04-13) |
| Vulkan build | `build/` — `GGML_VULKAN=ON`, `GGML_SYCL=OFF`, Release, native |
| SYCL build | `build-sycl/` — `GGML_SYCL=ON`, `GGML_SYCL_F16=ON`, icx/icpx, oneAPI 2025.3 |
| Old build preserved at | `build.stale-pre-20260821/` (April binaries, rollback) |

### Models

Both verified byte-exact against the Hub and confirmed to carry GGUF magic.

| File | Bytes | Role |
|---|---|---|
| `~/models/beamer/s1-mini-q4_k_m.gguf` | 484,219,808 | Stage 1 cleanup |
| `~/models/beamer/gemma-4-E4B_q4_0-it.gguf` | 5,154,941,280 | Stage 2 extraction |

Licences recorded in `licenses/`; required attribution added to `README.md`.
Gemma's repo ships **no** LICENSE file — its card declares Apache-2.0 in both
front matter and body; `licenses/gemma-4-LICENSE.txt` records that provenance.

## Finding 1 — B60 is Vulkan device 1, and the mask re-indexes it to 0

```
Vulkan0: Intel(R) Arc(tm) B570 Graphics (BMG G21)      (10172 MiB)
Vulkan1: Intel(R) Arc(tm) Pro B60 Graphics (BMG G21)   (24480 MiB)   <-- target
Vulkan2: AMD Ryzen 9 7950X3D (RADV RAPHAEL_MENDOCINO)  (31691 MiB)
```

With `GGML_VK_VISIBLE_DEVICES=1`, llama.cpp reports `Found 1 Vulkan devices: 0 = ...B60`.
**The mask value (1) and the in-process device id (0) are different numbers.**
This is the same shape as the `ZE_AFFINITY_MASK` confusion recorded in the
machine notes, where filtering to one device produced a misleading
"device count is zero" symptom. Do not conflate the two.

## Finding 2 — Vulkan drives the B60's matrix engines

```
ggml_vulkan: 0 = Intel(R) Arc(tm) Pro B60 Graphics (BMG G21) (Intel open-source Mesa driver)
  | uma: 0 | fp16: 1 | bf16: 1 | warp size: 32 | shared memory: 49152
  | int dot: 1 | matrix cores: NV_coopmat2
```

`matrix cores: NV_coopmat2` means Mesa's Intel driver exposes cooperative-matrix
v2 and llama.cpp compiles its `mul_mm_cm2` path against it. The historical reason
to prefer SYCL on Intel — Vulkan having no matrix-core path — no longer applies.
This does not establish which backend is faster; it establishes that they now
compete on comparable footing. Hence both builds exist, and the decision is
deferred to measurement below.

## Finding 3 — the router server already implements per-model idle shutdown

`--sleep-idle-seconds N` is a per-model-instance setting, and "sleep" genuinely
releases VRAM. From `tools/server/server-context.cpp:906`:

```cpp
void handle_sleeping_state(bool new_state) {
    if (new_state) { destroy(); }                 // frees model + context (VRAM)
    else           { load_model(params_base); }   // full reload, on next request
}
```

In router mode the parent spawns **one child `llama-server` per model**, each
rendered from a preset INI (`preset.to_args(bin_path)`), so any ordinary server
CLI argument can be set per model. Therefore the spec's "keep the small model
hot, unload the big one" is configuration, not Beamer code:

```ini
version = 1
[*]
c = 8192
ngl = 99
jinja = true

[s1-mini-q4_k_m]
sleep-idle-seconds = -1     ; never sleep — stays hot
load-on-startup = true

[gemma-4-E4B_q4_0-it]
sleep-idle-seconds = 300    ; release ~5 GB after 5 min idle
load-on-startup = false
```

Consequences for the design:

- Beamer does **not** need to call `POST /models/unload`, poll for load state,
  or implement idle timers. Deleted before it was written.
- `pin` (never evict) is **commented out** upstream, but with exactly two models
  and `--models-max 2` nothing is ever evicted for capacity, so it is not needed.
- Section names must match the model names `GET /models` reports. Assumed to be
  the filename stem; **unverified** until a server is running.
- The number that matters becomes **wake-from-sleep latency** (a model reload in
  a live process) rather than warm process respawn. The process persists, so
  backend init — including SYCL's first-run JIT — is paid once at startup, not
  per wake.

## Architecture decision (2026-08-21) — standalone server, Beamer is a client

llama.cpp runs as a **separate long-lived local server**; Beamer is a pure HTTP
client to it. Beamer does not spawn or supervise the process.

This supersedes the spawn-and-supervise design in the spec, and it retires that
document's stated reason for rejecting SYCL ("Vulkan needs no oneAPI environment
sourced, which matters for a process Beamer spawns from a desktop session") — a
service unit can set the oneAPI environment declaratively. Backend choice is
therefore reopened and settled by measurement, not by the spawn constraint.

Beamer must degrade gracefully when the server is absent: no cleanup pass, note
still captured. A sticky note must never be lost because a model server is down.

## Measurements

### Backend comparison — `llama-bench`, B60, commit `5a32f7b66`

Both backends built from the **same commit**, run on the **same device**, pinned
with explicit `-dev` (not env masks, so no index re-mapping), `-ngl 99`, `-r 3`.

| Model | Metric | Vulkan | SYCL | SYCL advantage |
|---|---|---:|---:|:--:|
| s1-mini-q4_k_m | pp512 | 6358.04 ± 16.53 | **14817.15 ± 45.00** | **2.33x** |
| s1-mini-q4_k_m | tg128 | 249.70 ± 1.70 | **293.70 ± 0.14** | **1.18x** |
| gemma-4-E4B_q4_0 | pp512 | 1190.80 ± 2.98 | **2811.44 ± 7.70** | **2.36x** |
| gemma-4-E4B_q4_0 | tg128 | 61.20 ± 0.09 | **77.41 ± 0.01** | **1.26x** |

**SYCL wins every cell.** Prompt processing is the decisive gap (~2.35x);
generation is smaller but consistent (~1.2x). Variance is negligible.

### Finding 4 — Vulkan on current master uses KHR_coopmat, not NV_coopmat2

The April build reported `matrix cores: NV_coopmat2`; commit `5a32f7b66`
reports `KHR_coopmat` on the same hardware. This is **not** a build defect:
configure confirms `GL_NV_cooperative_matrix2 supported by glslc`, so the
coopmat2 shaders are compiled in. Current master gates coopmat2 at runtime on a
longer device-feature list (`ggml-vulkan.cpp:7297`), now including
`cooperativeMatrixTensorAddressing` and `cooperativeMatrixBlockLoads`, which
Mesa's Intel driver does not fully satisfy. llama.cpp therefore falls back to
the v1 KHR path.

`KHR_coopmat` is still matrix-core (XMX) acceleration, not a generic FP shader
fallback — but it is the v1 path. Benchmarking it is correct, because it is what
the deployed configuration actually selects.

Open question, cheap to answer if ever needed: `build.stale-pre-20260821/` still
holds April binaries that *do* select coopmat2, so "would v2 have closed the
2.35x gap?" can be tested without rebuilding. Not pursued — SYCL's margin is
large and coopmat2 is unavailable on this driver regardless.

### Caveat on scope

`llama-bench` measures raw model throughput, not end-to-end request latency.
Server-level numbers (TTFT, wall-clock for a real note, cold start,
wake-from-sleep) are measured separately below.

### Server-level latency — SYCL, router mode, preset INI

Measured against the standalone router (`deploy/llama-models.ini`), the same way
Beamer will call it. Wall-clock is the full HTTP round trip.

#### Stage 1 — S1-mini cleanup (stays resident)

| Case | Wall-clock | Notes |
|---|---:|---|
| First request after load | 0.938 s | SYCL kernel warmup; one-off, not representative |
| Short note (30 tokens out) | **0.120 s** | steady state, 5 runs, +/- 0.001 s |
| ~60-word note (59 tokens out) | **0.225 s** | the spec's reference size |
| Filler-only (`um uh hmm`) | **0.049 s** | returns empty string, no hallucination |

Generation holds ~282 t/s across runs. Output quality is correct: fillers
removed, truecasing applied, `tuesdays` -> `Tuesday's`, and `Structure: lists`
correctly produces Markdown bullets.

> Note: repeat runs of an identical prompt report `prompt_n=1` — llama.cpp is
> reusing the KV cache. The honest per-note figure is the first run of a given
> text (72 prompt tokens at 4175 t/s ~= 17 ms), not the cached repeats.

#### Stage 2 — Gemma 4 E4B extraction (sleeps when idle)

| Case | Wall-clock | Notes |
|---|---:|---|
| Load (process spawn + 4.8 GB, page cache warm) | **4.06 s** | reproduced twice |
| Warm trivial request | **0.063 s** | |
| Extraction, thinking OFF | **1.11 s** | 57 tokens generated |
| Extraction, thinking ON | **4.46 s** | 335 tokens, *identical* extraction |

VRAM on the B60: 22317 MiB free idle -> 18898 MiB with Gemma loaded (~3.4 GB).

#### Finding 5 — both models need explicit anti-reasoning configuration

Neither model works correctly out of the box, and they fail differently. This is
the single highest-risk detail in the whole stack, because both failures look
like a working server returning a valid response.

**S1-mini** inherits Qwen3's chat template, which defaults thinking ON; the
model was trained with it OFF. Without `chat-template-kwargs
{"enable_thinking":false}` it emits `<think>` and stops after 3 tokens. The
model card also warns the GGUF carries `temp 0.6 / top_p 0.95 / top_k 20`
inherited from Qwen3-0.6B; S1-mini is trained for greedy decoding. The card
explicitly says NOT to substitute `reasoning-budget = 0` — it suppresses the
think block differently and fillers survive into the output.

**The Phase 1 plan's Task 1 Step 4 command is wrong on this point** — it passes
`--jinja` without `--chat-template-kwargs`, which reproduces the failure exactly.

**Gemma 4** reasons by default, filling `reasoning_content` while `content`
stays empty. With `max_tokens: 200` the first extraction call returned
`finish_reason: length` and no content at all, having spent the entire budget
thinking.

Thinking ON produced identical extraction output to thinking OFF on the one test
case, at 4x the latency. Default is OFF. Whether reasoning improves *precision*
on hard cases is a Phase 3 question that needs the eval corpus; n=1 settles
nothing about quality.

#### Extraction precision spot-check

The test note contained a planted aspiration ("It would be nice if someone
eventually redesigned the onboarding flow") alongside four first-person
commitments. Both thinking modes extracted exactly the four commitments and
excluded the aspiration — the behaviour the spec's strict policy requires.
One sample; indicative only.

### Wake-from-sleep — the number the idle-shutdown design rests on

Measured with `sleep-idle-seconds = 60` (same mechanism as the 300 s default,
shorter timer). Sleep is confirmed in the child log:

```
[51689] I que start_loop:  entering sleeping state
[51689] I srv handle_sleep: server is entering sleeping state
[51689] I que start_loop:  exiting sleeping state
```

| State | B60 VRAM free |
|---|---:|
| Gemma loaded | 18898 MiB |
| Gemma sleeping | **21985 MiB** |
| **Released by sleep** | **3087 MiB** |

| Transition | Wall-clock |
|---|---:|
| Cold — process spawn + 4.8 GB load (page cache warm) | 4.06 s |
| **Wake from sleep** | **1.68 s** |
| Warm request (awake) | 0.063 s |

Sleep genuinely frees VRAM, and waking is **2.4x cheaper than respawning**
(1.68 s vs 4.06 s) because the process, its SYCL context and its kernels
survive — only the model and llama context are rebuilt.

Asymmetry confirmed working: with `sleep-idle-seconds = -1`, S1-mini stayed
`loaded` through the same idle period in which Gemma went `sleeping`.

#### Finding 6 — polling the server defeats the idle timer

The idle timer did not fire while `GET /v1/models` was being polled every 3 s to
watch for it. It fired within 100 s of genuine silence. Status reads count as
activity.

**This is a live constraint on Beamer, not a test artifact.** A naive health
poll on a short interval will pin the ~3 GB extraction model in VRAM forever and
silently defeat the idle-shutdown design — with no error and no symptom other
than VRAM never coming back. Beamer must either poll lazily (only when it is
about to make a request) or not at all, treating a connection failure as the
health signal instead.

### Cold start (page cache dropped)

NOT MEASURED — requires `sudo sync && echo 3 > /proc/sys/vm/drop_caches`, which
needs the user. All load figures above are page-cache-warm. This matters only
for the first load after a reboot; the design rests on wake-from-sleep, which is
measured.

## Verdict

**Backend: SYCL.** Settled by measurement, not by the spawn-constraint argument
in the spec. SYCL is 2.35x faster at prompt processing and ~1.2x at generation
on both models, from the same commit on the same device. The committed decision
to use Vulkan was made on an operational constraint (no oneAPI environment for a
Beamer-spawned process) that the standalone-server architecture removed; with
that gone, the performance data decides, and it is not close.

Vulkan remains a working fallback (`build/`) requiring no oneAPI runtime.

**Phase 2 (S1-mini cleanup): projected to pass, pending server-level
confirmation.** At 14817 t/s prompt and 294 t/s generation, a ~60-word note
(~120 prompt tokens, ~80 generated) projects to roughly 0.3 s of model time,
far inside the 1.5 s threshold. This is a projection from throughput, not a
measurement; the server-level number supersedes it.

**Asymmetric idle shutdown: CONFIRMED CORRECT.** Wake from sleep is **1.68 s**,
far inside the >10 s threshold that would have condemned it, and sleep really
returns 3087 MiB. Keep the default: S1-mini resident (`-1`), Gemma sleeping
after 300 s.

**Phase 2 (S1-mini cleanup): PASSES.** A ~60-word note cleans in **0.225 s**
against a 1.5 s threshold — a 6.7x margin. Measured end-to-end over HTTP against
the real server with the real prompt format, not projected. Phase 2 proceeds as
specced; no redesign needed.

**Phase 3 (extraction): viable, precision unproven.** 1.11 s per extraction with
thinking off. The one precision spot-check behaved correctly. Quality remains an
open question that only the eval corpus can answer, exactly as the spec says.

## Consequences for the plan and spec

1. **Task 1 Step 4's command is wrong** — it omits
   `--chat-template-kwargs '{"enable_thinking":false}'` and reproduces the
   `<think>`-and-stop failure. Fix before anyone reruns it.
2. **Backend flips from Vulkan to SYCL**, on measurement.
3. **The spawn-and-supervise subsystem is deleted** — standalone server,
   Beamer is an HTTP client.
4. **Per-model idle shutdown is configuration, not code** (`deploy/llama-models.ini`).
5. **Beamer must not poll the server on a timer** (Finding 6).
6. **Beamer must degrade gracefully** when the server is down: capture the note,
   skip the cleanup.

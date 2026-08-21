# B60 llama.cpp benchmark — Task 1

> **Status: IN PROGRESS.** Environment findings are recorded and verified.
> Latency/throughput measurements are not yet taken; the verdict section at the
> bottom is unanswered until they are.

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

> NOT YET TAKEN. The Vulkan build is blocked on `sudo apt install spirv-headers`
> (a new hard dependency of the Vulkan backend on current master).

Two single-token smoke runs were observed while validating the toolchains. They
are **not** comparable and must not be read as a result — different build
commits, different `ngl`, and `tg1` is dominated by fixed overhead:

| Backend | Build | ngl | tg1 t/s |
|---|---|---|---|
| Vulkan | `e97492369` (April) | 99 | 136.34 ± 9.76 |
| SYCL | `5a32f7b66` (today) | -1 (default) | 229.34 ± 18.37 |

Real measurements to take, both backends, both models: prompt-processing t/s,
token-generation t/s, time-to-first-token, cold start, warm start, and
wake-from-sleep.

## Verdict

UNANSWERED — pending measurements.

Thresholds to apply, from the plan:
- S1-mini cleans a ~60-word note in **≲1.5 s** → Phase 2 proceeds as specced.
- Wake-from-sleep of E4B **>10 s** → asymmetric idle-shutdown default is wrong;
  the model should stay resident.

# Sticky Notes — Phase 1 (Capture & Desktop Notes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A second global hotkey dictates into a sticky note that appears on the GNOME desktop, persists across restarts, and remembers where it was placed.

**Architecture:** The existing hotkey → audio → ASR pipeline gains a `CaptureMode` that selects its terminal sink: inject into the focused field (today's behaviour) or create a note. Notes are plain Dioxus windows, positioned via a new D-Bus method on Beamer's own GNOME Shell extension, since Wayland forbids clients from placing or reading their own windows. Notes persist as JSON with debounced writes.

**Tech Stack:** Rust, Dioxus 0.7 desktop, evdev (Linux hotkeys), zbus (D-Bus), GJS/St (GNOME extension), serde_json, chrono.

**Spec:** `docs/superpowers/specs/2026-08-20-sticky-notes-design.md`

## Global Constraints

- **Every file stays under 500 lines.** Split before approaching the limit. (`CLAUDE.md`)
- **Never create a second tokio runtime.** Dioxus 0.7 owns the main thread and the only runtime. Use `spawn()` inside hooks. (`agent_docs/dioxus_architecture.md`)
- **All blocking work — D-Bus, process spawn, Win32/UIA — goes through `tokio::task::spawn_blocking`.**
- **Design language is Deploy Purple:** purple accent, 2px borders, hard-offset shadows, solid backgrounds. (`agent_docs/design_system.md`)
- **No `Co-Authored-By: Claude` trailers** in any commit. (user's global `CLAUDE.md`)
- **Attribution, legally required:** the string `"S1-mini" by "Superwhisper"` — that exact capitalization — must appear in the repo README and in an in-app credits surface once the model is in use. This is a licence term, not a courtesy.
- **Tests use the existing repo pattern:** inline `#[cfg(test)] mod tests`, PID-scoped temp dirs so concurrent runs don't race and nothing touches the real config dir. See `src/ui/history.rs` tests.
- **Persistence writes atomically:** write to a `.tmp` sibling, then rename over the target. See `TranscriptionHistory::save`.
- **GNOME extensions do not hot-reload on Wayland.** Any extension change requires a full log out and back in to test.

## Phase scope

Phase 1 delivers dictation → desktop sticky notes with no AI. Phases 2 (S1-mini cleanup) and 3 (task extraction, suggestion chips, Tasks page) get their own plans, because their content depends on measurements that do not exist yet: Task 1's benchmark numbers, and an eval corpus that can only be built once Phase 1 is capturing real notes.

---

### Task 1: Benchmark the models on the B60 before writing any feature code

The spec makes this step 1 deliberately: every latency figure in it is an estimate, and if stage 1 cannot clean a note in roughly a second the "watch the sticky tidy itself" experience does not exist and Phases 2–3 need redesigning. Better to learn that now than after ten tasks of UI work.

**Files:**
- Create: `docs/superpowers/benchmarks/2026-08-20-b60-llama-vulkan.md`
- Create: `licenses/S1-mini-LICENSE.txt`
- Create: `licenses/gemma-4-LICENSE.txt`
- Modify: `README.md` (attribution section)

**Interfaces:**
- Consumes: nothing.
- Produces: measured tokens/sec, time-to-first-token, and cold/warm server start times for both models, recorded in the benchmark doc. Phase 2's plan is written against these numbers.

- [ ] **Step 1: Confirm llama-server sees the B60 and note its Vulkan index**

Vulkan device order is not guaranteed to match DRM card order, so this must be read, not assumed.

```bash
export LD_LIBRARY_PATH=/home/berkley/Programming/llama.cpp/build/bin:$LD_LIBRARY_PATH
/home/berkley/Programming/llama.cpp/build/bin/llama-server --list-devices
```

Expected: a list including both `Intel(R) Arc(tm) B570 Graphics` and `Intel(R) Arc(tm) Pro B60 Graphics`. **Record the index of the B60** — it is the value for `llm.vulkan_device` in config. Do not assume it is 0.

- [ ] **Step 2: Download both models**

```bash
mkdir -p ~/models/beamer
pip install --user -q huggingface_hub[cli] 2>/dev/null || true
hf download superwhisper/s1-mini-GGUF s1-mini-q4_k_m.gguf \
  --local-dir ~/models/beamer
hf download superwhisper/s1-mini-GGUF LICENSE --local-dir ~/models/beamer
hf download google/gemma-4-E4B-it-qat-q4_0-gguf gemma-4-E4B_q4_0-it.gguf \
  --local-dir ~/models/beamer
```

Expected: `s1-mini-q4_k_m.gguf` (~462 MB) and `gemma-4-E4B_q4_0-it.gguf` (~5.15 GB). Do **not** download either `mmproj` file — Beamer's use is text-only.

- [ ] **Step 3: Record the licences and the required attribution**

Copy the downloaded `LICENSE` to `licenses/S1-mini-LICENSE.txt`. Fetch Gemma 4's licence from its repo into `licenses/gemma-4-LICENSE.txt`, and **read it** to confirm the Apache-2.0 tag on the Hub matches the file — some third-party Gemma derivatives are tagged `license:gemma` instead.

Add to `README.md`:

```markdown
## Model credits

Beamer's on-device transcript cleanup uses "S1-mini" by "Superwhisper"
(https://huggingface.co/superwhisper/s1-mini), used under the Apache License
2.0 with the additional naming term recorded in `licenses/S1-mini-LICENSE.txt`.

Task extraction uses Google's Gemma 4 (`gemma-4-E4B-it`), used under the terms
in `licenses/gemma-4-LICENSE.txt`.
```

The S1-mini string must be exactly `"S1-mini" by "Superwhisper"`, quotes and capitalization included. That is what the licence requires.

- [ ] **Step 4: Benchmark stage 1 (S1-mini) with its real prompt format**

The measurement is worthless with a made-up prompt: S1-mini was trained on an exact input shape and produces garbage without it. Use the real one.

> **CORRECTED 2026-08-21.** The original command here omitted
> `--chat-template-kwargs`, which makes S1-mini emit `<think>` and stop after
> three tokens. S1-mini inherits Qwen3's chat template, which defaults thinking
> ON; the model was trained with it OFF. Do **not** substitute
> `--reasoning-budget 0` — the model card says output degrades. The GGUF also
> carries `temp 0.6 / top_p 0.95 / top_k 20` inherited from Qwen3-0.6B, so set
> greedy decoding explicitly. See
> `docs/superpowers/benchmarks/2026-08-20-b60-llama-vulkan.md` Finding 5.
>
> Note also that the backend is now **SYCL** (`build-sycl/`), not Vulkan, and
> that this whole step is superseded in practice by the standalone router
> server in `deploy/`. Kept here for the record.

```bash
export LD_LIBRARY_PATH=/home/berkley/Programming/llama.cpp/build/bin:$LD_LIBRARY_PATH
/home/berkley/Programming/llama.cpp/build/bin/llama-server \
  -m ~/models/beamer/s1-mini-q4_k_m.gguf \
  --port 8081 --host 127.0.0.1 -dev Vulkan1 -ngl 99 -c 8192 --jinja \
  --chat-template-kwargs '{"enable_thinking":false}' --temp 0 --top-k 1 &
```

Time how long `/health` takes to return ready, then:

```bash
time curl -s http://127.0.0.1:8081/v1/chat/completions \
  -H 'Content-Type: application/json' -d '{
  "temperature": 0,
  "messages": [
    {"role":"system","content":"You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text."},
    {"role":"user","content":"[Styling: semi-formal] [Structure: lists] [Context: general]\num so i need to like call the vet about milo and uh also send tuesdays invoice and i guess pick up the dry cleaning at some point"}
  ]}' | tee /tmp/s1_out.json
```

Expected: cleaned, punctuated text. Record wall-clock latency, and `timings.predicted_per_second` from the response.

Also test the empty case — this must return an empty string, not a hallucination:

```bash
curl -s http://127.0.0.1:8081/v1/chat/completions -H 'Content-Type: application/json' -d '{
  "temperature": 0,
  "messages": [
    {"role":"system","content":"You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text."},
    {"role":"user","content":"[Styling: semi-formal] [Structure: prose] [Context: general]\num uh hmm"}
  ]}'
```

- [ ] **Step 5: Benchmark stage 2 (Gemma 4 E4B) cold and warm start**

```bash
kill %1
sync && echo 3 | sudo tee /proc/sys/vm/drop_caches   # cold: defeat page cache
GGML_VK_VISIBLE_DEVICES=<B60_INDEX> \
/home/berkley/Programming/llama.cpp/build/bin/llama-server \
  -m ~/models/beamer/gemma-4-E4B_q4_0-it.gguf \
  --port 8082 --host 127.0.0.1 -ngl 99 -c 8192 --jinja &
```

Time `/health` to ready. Kill it, restart **without** dropping caches, and time it again. The warm figure is the one that matters — the spec's asymmetric idle-shutdown design (extraction model unloads after 5 min) rests on a warm respawn being cheap. Record both.

Then measure a representative extraction call and record `predicted_per_second`.

- [ ] **Step 6: Record findings and judge the design**

Write `docs/superpowers/benchmarks/2026-08-20-b60-llama-vulkan.md` with: B60 Vulkan index, both models' cold/warm start times, tokens/sec, time-to-first-token, and the cleaned/empty outputs observed.

Then state a verdict explicitly:

- If S1-mini cleans a ~60-word note in **≲1.5 s**, Phase 2 proceeds as specced.
- If it is much slower, Phase 2 needs redesign — say so in the doc rather than proceeding quietly.
- If warm respawn of E4B is **>10 s**, the asymmetric idle-shutdown default is wrong and should become resident; record that.

- [ ] **Step 7: Commit**

```bash
git add docs/superpowers/benchmarks/ licenses/ README.md
git commit -m "bench: measure S1-mini and Gemma 4 E4B on the B60 via Vulkan"
```

---

### Task 2: Thread `CaptureMode` through `HotkeyEvent`

The smallest possible change that gives the orchestrator a way to know which hotkey fired. Everything downstream depends on this type, so it lands first and alone.

**Files:**
- Modify: `src/hotkey/mod.rs`
- Modify: `src/orchestrator/mod.rs` (match arms only)
- Modify: `src/hotkey/linux_hotkey.rs` (call sites only — full support in Task 3)

**Interfaces:**
- Produces:
  - `pub enum CaptureMode { Inject, Note }` — `Copy`, `Clone`, `Debug`, `PartialEq`, `Eq`
  - `pub enum HotkeyEvent { RecordStart(CaptureMode), RecordStop }`

- [ ] **Step 1: Write the failing test**

Add to the bottom of `src/hotkey/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_start_carries_its_capture_mode() {
        let inject = HotkeyEvent::RecordStart(CaptureMode::Inject);
        let note = HotkeyEvent::RecordStart(CaptureMode::Note);

        assert_ne!(
            inject, note,
            "the orchestrator must be able to tell the two hotkeys apart"
        );
        match note {
            HotkeyEvent::RecordStart(mode) => assert_eq!(mode, CaptureMode::Note),
            HotkeyEvent::RecordStop => panic!("wrong variant"),
        }
    }

    #[test]
    fn capture_mode_defaults_to_inject() {
        assert_eq!(
            CaptureMode::default(),
            CaptureMode::Inject,
            "an unconfigured note hotkey must never silently divert dictation"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer hotkey::tests 2>&1 | tail -20`
Expected: FAIL — `cannot find type CaptureMode`, and `HotkeyEvent::RecordStart` takes no arguments.

- [ ] **Step 3: Write minimal implementation**

In `src/hotkey/mod.rs`, replace the `HotkeyEvent` definition:

```rust
/// Which sink a recording session's final transcript should reach.
///
/// Beamer has two dictation hotkeys that share the entire audio and ASR
/// pipeline and differ only in what happens to the finished text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureMode {
    /// Inject into the focused input field — the original behaviour.
    #[default]
    Inject,
    /// Create a sticky note.
    Note,
}

/// Sent from the hotkey listener to the orchestrator coroutine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    RecordStart(CaptureMode),
    RecordStop,
}
```

Fix the resulting compile errors by pattern-matching the payload and ignoring it for now — Task 7 gives it meaning:

- `src/orchestrator/mod.rs`: change `HotkeyEvent::RecordStart =>` to `HotkeyEvent::RecordStart(_) =>` at every match site (there are several; the compiler lists them all).
- `src/hotkey/linux_hotkey.rs`: change every `HotkeyEvent::RecordStart` construction to `HotkeyEvent::RecordStart(CaptureMode::Inject)` and add `CaptureMode` to the `use crate::hotkey::{...}` import.

- [ ] **Step 4: Run tests and build to verify**

Run: `cargo test --bin beamer hotkey::tests 2>&1 | tail -20`
Expected: PASS, both tests.

Run: `cargo build 2>&1 | tail -20`
Expected: builds clean with no warnings about unreachable patterns.

- [ ] **Step 5: Commit**

```bash
git add src/hotkey/mod.rs src/orchestrator/mod.rs src/hotkey/linux_hotkey.rs
git commit -m "feat(hotkey): carry CaptureMode on RecordStart"
```

---

### Task 3: Linux evdev listener watches two hotkeys

`HookState` currently tracks one `HotkeyConfig` and one set of press-state flags. Two bindings need two independent sets of runtime state (a hold-to-talk inject binding and a toggle note binding must not corrupt each other), while modifier state stays shared.

**Files:**
- Modify: `src/hotkey/linux_hotkey.rs`

**Interfaces:**
- Consumes: `CaptureMode`, `HotkeyEvent::RecordStart(CaptureMode)` from Task 2.
- Produces:
  - `HotkeyHandle::update_configs(&self, inject: HotkeyConfig, note: Option<HotkeyConfig>)`
  - `start_ll_hook(inject: HotkeyConfig, note: Option<HotkeyConfig>, tx: UnboundedSender<HotkeyEvent>) -> HotkeyHandle`

  `note: None` means no note hotkey is configured, and no note events are ever emitted.

- [ ] **Step 1: Write the failing test**

Add to the bottom of `src/hotkey/linux_hotkey.rs`. This tests the pure decision logic, not evdev I/O.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(ctrl: bool, shift: bool, vk: u32, toggle: bool) -> HotkeyConfig {
        HotkeyConfig { ctrl, alt: false, shift, trigger_vk: vk, is_toggle: toggle }
    }

    /// Ctrl+Space -> inject (hold), Ctrl+Shift+N -> note (toggle).
    fn two_bindings() -> Vec<BindingConfig> {
        vec![
            BindingConfig { mode: CaptureMode::Inject, config: cfg(true, false, 0x20, false) },
            BindingConfig { mode: CaptureMode::Note,   config: cfg(true, true,  0x4E, true) },
        ]
    }

    #[test]
    fn each_binding_matches_only_its_own_chord() {
        let bindings = two_bindings();

        // Ctrl held, Shift not: Space matches inject, N matches nothing.
        let mods = Modifiers { ctrl: true, alt: false, shift: false };
        assert_eq!(matching_binding(&bindings, 0x20, mods), Some(0));
        assert_eq!(matching_binding(&bindings, 0x4E, mods), None);

        // Ctrl+Shift held: N matches note, Space matches nothing.
        let mods = Modifiers { ctrl: true, alt: false, shift: true };
        assert_eq!(matching_binding(&bindings, 0x4E, mods), Some(1));
        assert_eq!(
            matching_binding(&bindings, 0x20, mods), None,
            "Ctrl+Shift+Space must not trigger the plain Ctrl+Space binding"
        );
    }

    #[test]
    fn bindings_keep_independent_press_state() {
        let mut state = [BindingState::default(); MAX_BINDINGS];

        state[0].trigger_held = true;
        state[0].armed = true;

        assert!(!state[1].trigger_held, "note binding must not inherit inject's held state");
        assert!(!state[1].armed, "note binding must not inherit inject's armed state");
    }

    #[test]
    fn absent_note_binding_yields_only_one_binding() {
        let bindings = build_bindings(cfg(true, false, 0x20, false), None);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].mode, CaptureMode::Inject);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer linux_hotkey 2>&1 | tail -20`
Expected: FAIL — `BindingConfig`, `Modifiers`, `matching_binding`, `BindingState`, `MAX_BINDINGS`, `build_bindings` are all undefined.

- [ ] **Step 3: Write minimal implementation**

In `src/hotkey/linux_hotkey.rs`, add these types and helpers above `HookState`:

```rust
/// Beamer has exactly two dictation hotkeys: inject and note.
pub(super) const MAX_BINDINGS: usize = 2;

/// One configured hotkey and the sink it selects.
#[derive(Clone)]
pub(super) struct BindingConfig {
    pub mode: CaptureMode,
    pub config: HotkeyConfig,
}

/// Per-binding press state. Kept separate from `BindingConfig` because the
/// config is swapped wholesale by `update_configs` while press state must
/// survive — a user editing the note hotkey mid-hold shouldn't strand the
/// inject binding in `armed`.
#[derive(Clone, Copy, Default)]
pub(super) struct BindingState {
    pub armed: bool,
    pub toggled_on: bool,
    pub trigger_held: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

pub(super) fn build_bindings(
    inject: HotkeyConfig,
    note: Option<HotkeyConfig>,
) -> Vec<BindingConfig> {
    let mut v = vec![BindingConfig { mode: CaptureMode::Inject, config: inject }];
    if let Some(note) = note {
        v.push(BindingConfig { mode: CaptureMode::Note, config: note });
    }
    v
}

/// Index of the binding whose trigger key and modifier set both match, or
/// `None`. Modifiers must match *exactly*, so Ctrl+Shift+Space does not fire a
/// binding registered for plain Ctrl+Space.
pub(super) fn matching_binding(
    bindings: &[BindingConfig],
    vk: u32,
    mods: Modifiers,
) -> Option<usize> {
    bindings.iter().position(|b| {
        b.config.trigger_vk == vk
            && b.config.ctrl == mods.ctrl
            && b.config.alt == mods.alt
            && b.config.shift == mods.shift
    })
}
```

Change `HookState` to hold both:

```rust
struct HookState {
    bindings: Arc<Mutex<Vec<BindingConfig>>>,
    reset_flag: Arc<AtomicBool>,
    tx: UnboundedSender<HotkeyEvent>,
    ctrl_held: bool,
    alt_held: bool,
    shift_held: bool,
    binding_state: [BindingState; MAX_BINDINGS],
}
```

Rewrite the trigger-handling body (the block read in Step 1 of this task's file, which currently locks `state.config`) to resolve the binding first:

```rust
    let mods = Modifiers {
        ctrl: state.ctrl_held,
        alt: state.alt_held,
        shift: state.shift_held,
    };

    let bindings = state.bindings.lock().unwrap().clone();

    // The Win-key trigger is matched by keycode rather than VK because evdev
    // reports left/right meta separately.
    let vk = if matches!(key, KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA) {
        Some(VK_LWIN)
    } else {
        evdev_key_to_vk(key)
    };
    let Some(vk) = vk else { return };

    // On release we must find the binding that is actually held: the modifiers
    // may already be up by the time the trigger key is released, so matching on
    // the chord again would find nothing and strand the binding in `armed`.
    let idx = if is_press {
        matching_binding(&bindings, vk, mods)
    } else {
        (0..bindings.len())
            .find(|&i| bindings[i].config.trigger_vk == vk && state.binding_state[i].trigger_held)
    };
    let Some(idx) = idx else { return };

    let binding = &bindings[idx];
    let mode = binding.mode;
    let is_toggle = binding.config.is_toggle;
    let bs = &mut state.binding_state[idx];

    if is_press {
        if !bs.trigger_held {
            bs.trigger_held = true;
            if is_toggle {
                bs.toggled_on = !bs.toggled_on;
                let event = if bs.toggled_on {
                    tracing::info!("Hotkey triggered: RecordStart({:?}) (toggle)", mode);
                    HotkeyEvent::RecordStart(mode)
                } else {
                    tracing::info!("Hotkey triggered: RecordStop (toggle)");
                    HotkeyEvent::RecordStop
                };
                let _ = state.tx.send(event);
            } else {
                tracing::info!("Hotkey triggered: RecordStart({:?}) (hold)", mode);
                let _ = state.tx.send(HotkeyEvent::RecordStart(mode));
                bs.armed = true;
            }
        }
    } else if bs.trigger_held {
        bs.trigger_held = false;
        if bs.armed {
            tracing::info!("Hotkey triggered: RecordStop (hold release)");
            let _ = state.tx.send(HotkeyEvent::RecordStop);
            bs.armed = false;
        }
    }
```

Update `start_ll_hook` and `HotkeyHandle`:

```rust
#[derive(Clone)]
pub struct HotkeyHandle {
    bindings: Arc<Mutex<Vec<BindingConfig>>>,
    reset_flag: Arc<AtomicBool>,
}

impl HotkeyHandle {
    pub fn update_configs(&self, inject: HotkeyConfig, note: Option<HotkeyConfig>) {
        *self.bindings.lock().unwrap() = build_bindings(inject, note);
        self.reset_flag.store(true, Ordering::Relaxed);
    }
}

pub fn start_ll_hook(
    inject: HotkeyConfig,
    note: Option<HotkeyConfig>,
    tx: UnboundedSender<HotkeyEvent>,
) -> HotkeyHandle {
    let bindings = Arc::new(Mutex::new(build_bindings(inject, note)));
    let reset_flag = Arc::new(AtomicBool::new(false));

    let state = Arc::new(Mutex::new(HookState {
        bindings: bindings.clone(),
        reset_flag: reset_flag.clone(),
        tx,
        ctrl_held: false,
        alt_held: false,
        shift_held: false,
        binding_state: [BindingState::default(); MAX_BINDINGS],
    }));

    // ... rest of the existing body unchanged ...

    HotkeyHandle { bindings, reset_flag }
}
```

Wherever `reset_flag` is consumed, clear **all** `binding_state` entries, not just one.

Update the caller in `src/ui/app.rs` (and anywhere else the compiler flags) to pass `None` for the note hotkey for now — Task 5 wires the config value.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --bin beamer linux_hotkey 2>&1 | tail -20`
Expected: PASS, all three tests.

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 5: Manually verify the existing hotkey still works**

Run: `RUST_LOG=beamer=debug cargo run 2>&1 | grep -i hotkey`

Press the configured dictation hotkey. Expected: `Hotkey triggered: RecordStart(Inject) (hold)` then `RecordStop`. **This is a regression check** — Task 3 rewrites the hot path for the feature that already works, and a unit test cannot prove evdev still delivers.

- [ ] **Step 6: Commit**

```bash
git add src/hotkey/linux_hotkey.rs src/ui/app.rs
git commit -m "feat(hotkey): evdev listener watches two independent bindings"
```

---

### Task 4: Windows low-level hook parity

`src/hotkey/ll_hook.rs` is `#[cfg(target_os = "windows")]`, so it is **not compiled on this machine and cannot be verified here.** Keep the change mechanical and mirror Task 3 exactly.

**Files:**
- Modify: `src/hotkey/ll_hook.rs`

**Interfaces:**
- Produces: the same `start_ll_hook(inject, note, tx) -> HotkeyHandle` and `HotkeyHandle::update_configs(inject, note)` signatures as Task 3, so `src/ui/app.rs` needs no `#[cfg]` at the call site.

- [ ] **Step 1: Read the current file and locate the equivalent structures**

Run: `grep -n "HookState\|trigger_held\|armed\|toggled_on\|RecordStart\|update_config\|pub fn start_ll_hook" src/hotkey/ll_hook.rs`

- [ ] **Step 2: Apply the same shape as Task 3**

Reuse the `BindingConfig`, `BindingState`, `Modifiers`, `MAX_BINDINGS`, `build_bindings` and `matching_binding` items. **Move them from `linux_hotkey.rs` into `src/hotkey/mod.rs`** so both platforms share one copy rather than duplicating the matching logic — the matching rule is platform-independent and duplicating it guarantees the two platforms drift.

Update `linux_hotkey.rs` to import them from the parent module, and keep its unit tests where they are (they test shared logic through the Linux module's re-export; that is fine).

Replace the single-config `HookState` fields with `bindings` + `binding_state: [BindingState; MAX_BINDINGS]`, and emit `HotkeyEvent::RecordStart(mode)` using the matched binding's mode.

- [ ] **Step 3: Verify it at least parses**

There is no Windows toolchain here, so a real build is not available. Run:

```bash
cargo build 2>&1 | tail -5
```

Expected: the Linux build still succeeds (proving the shared items moved cleanly and `linux_hotkey.rs` still compiles).

Then check the Windows file for obvious syntax damage:

```bash
rustfmt --check --edition 2021 src/hotkey/ll_hook.rs && echo "parses cleanly"
```

Expected: `parses cleanly`, or a diff of formatting-only changes. A parse error here means the edit is broken.

- [ ] **Step 4: Record the limitation**

Add to `docs/superpowers/plans/2026-08-20-sticky-notes-phase1.md` progress notes, or to the commit body: this file is **unverified** — it type-checks nowhere available and must be built on Windows before any release that claims Windows support.

- [ ] **Step 5: Commit**

```bash
git add src/hotkey/mod.rs src/hotkey/ll_hook.rs src/hotkey/linux_hotkey.rs
git commit -m "feat(hotkey): share binding-matching logic and mirror two bindings on Windows

The Windows low-level hook cannot be compiled or tested on the development
machine (Linux). The change mirrors the evdev implementation mechanically and
must be built on Windows before any release claiming Windows support."
```

---

### Task 5: Config — note hotkey and notes section

**Files:**
- Modify: `src/config/mod.rs`
- Modify: `agent_docs/config_schema.md`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `RecordingConfig::note_hotkey: String` (default `""` — unset)
  - `RecordingConfig::note_mode: String` (default `"toggle"`)
  - `Config::notes: NotesConfig` with `all_workspaces: bool` (default `true`) and `default_color: String` (default `"purple"`)
  - `pub fn note_hotkey_config(&self) -> Option<HotkeyConfig>` on `RecordingConfig`

- [ ] **Step 1: Write the failing test**

Add to `src/config/mod.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn note_hotkey_is_unset_by_default() {
    let cfg = RecordingConfig::default();
    assert_eq!(cfg.note_hotkey, "", "no default chord may be stolen from another app");
    assert!(
        cfg.note_hotkey_config().is_none(),
        "an unset note hotkey must produce no binding at all"
    );
}

#[test]
fn configured_note_hotkey_parses_to_a_binding() {
    let cfg = RecordingConfig {
        note_hotkey: "Ctrl+Shift+N".into(),
        note_mode: "toggle".into(),
        ..RecordingConfig::default()
    };
    let parsed = cfg.note_hotkey_config().expect("should parse");
    assert!(parsed.ctrl);
    assert!(parsed.shift);
    assert!(!parsed.alt);
    assert_eq!(parsed.trigger_vk, 0x4E); // N
    assert!(parsed.is_toggle, "note capture defaults to toggle, not hold");
}

#[test]
fn unparseable_note_hotkey_yields_no_binding_rather_than_a_wrong_one() {
    let cfg = RecordingConfig {
        note_hotkey: "Ctrl+NotAKey".into(),
        ..RecordingConfig::default()
    };
    assert!(cfg.note_hotkey_config().is_none());
}

#[test]
fn notes_config_defaults() {
    let cfg = NotesConfig::default();
    assert!(cfg.all_workspaces, "a sticky note should follow you across workspaces");
    assert_eq!(cfg.default_color, "purple");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer config:: 2>&1 | tail -20`
Expected: FAIL — no field `note_hotkey`, no `NotesConfig`.

- [ ] **Step 3: Write minimal implementation**

In `src/config/mod.rs`, extend `RecordingConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub pause_media: bool,
    /// Chord that dictates into a sticky note instead of injecting.
    /// Empty means unconfigured: no note binding is registered at all.
    #[serde(default)]
    pub note_hotkey: String,
    /// "toggle" or "hold". Toggle by default — a note is usually longer than
    /// a dictated phrase, and holding a chord through it is awkward.
    #[serde(default = "default_note_mode")]
    pub note_mode: String,
}

fn default_note_mode() -> String { "toggle".into() }
```

Update `impl Default for RecordingConfig` to include both new fields.

Add the accessor:

```rust
impl RecordingConfig {
    /// Parsed note-capture binding, or `None` when unconfigured or unparseable.
    ///
    /// Returning `None` on a bad value is deliberate: silently falling back to
    /// some other chord would bind dictation to a key the user never chose.
    pub fn note_hotkey_config(&self) -> Option<crate::hotkey::HotkeyConfig> {
        if self.note_hotkey.trim().is_empty() {
            return None;
        }
        crate::hotkey::HotkeyConfig::parse(&self.note_hotkey, self.note_mode == "toggle")
    }
}
```

Add the notes section:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotesConfig {
    /// Mutter only: `stick()` note windows so they follow across workspaces.
    #[serde(default = "default_true")]
    pub all_workspaces: bool,
    #[serde(default = "default_note_color")]
    pub default_color: String,
}

fn default_note_color() -> String { "purple".into() }

impl Default for NotesConfig {
    fn default() -> Self {
        Self { all_workspaces: true, default_color: default_note_color() }
    }
}
```

Add `#[serde(default)] pub notes: NotesConfig,` to `Config` and to its `Default` impl.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --bin beamer config:: 2>&1 | tail -20`
Expected: PASS, all four new tests plus the existing ones.

- [ ] **Step 5: Wire the note hotkey into the listener**

In `src/ui/app.rs`, where `start_ll_hook` is called, replace the `None` placeholder from Task 3:

```rust
let note_binding = config.peek().recording.note_hotkey_config();
let handle = crate::hotkey::start_ll_hook(inject_binding, note_binding, tx);
```

Do the same at every `update_configs` call site so editing the hotkey in settings takes effect live.

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 6: Update the schema doc**

Add the new keys to `agent_docs/config_schema.md`, documenting that `note_hotkey = ""` disables note capture entirely and that `note_mode` accepts `"toggle"` or `"hold"`.

- [ ] **Step 7: Commit**

```bash
git add src/config/mod.rs src/ui/app.rs agent_docs/config_schema.md
git commit -m "feat(config): note hotkey and notes section"
```

---

### Task 6: Notes store

Pure Rust with no UI or GPU dependency — the most thoroughly testable part of the feature.

**Files:**
- Create: `src/notes/mod.rs`
- Modify: `src/main.rs` (add `mod notes;`)

**Interfaces:**
- Consumes: `Config::config_dir()`.
- Produces:
  - `pub enum NoteState { Raw, Cleaned, CleanFailed, Analyzed, ExtractFailed }`
  - `pub enum NoteColor { Purple, Violet, Amber, Teal, Rose, Slate }` with `NoteColor::from_config_name(&str) -> NoteColor`
  - `pub struct Note { id, created, modified, raw, body, state, color, pos, size, open, archived }`
  - `pub struct NoteStore` with:
    - `load() -> NoteStore`
    - `create(&mut self, raw: String, color: NoteColor) -> String` (returns the new id)
    - `get(&self, id: &str) -> Option<&Note>`
    - `set_body(&mut self, id: &str, body: String)`
    - `set_geometry(&mut self, id: &str, pos: (i32,i32), size: (u32,u32))`
    - `set_open(&mut self, id: &str, open: bool)`
    - `archive(&mut self, id: &str)`
    - `active(&self) -> Vec<&Note>` (non-archived, newest first)
    - `flush_if_dirty(&mut self) -> bool` (true when it wrote)
    - `save(&self) -> Result<()>`

- [ ] **Step 1: Write the failing test**

Create `src/notes/mod.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// PID-scoped temp path so concurrent test runs don't race and nothing
    /// touches the real user config dir. Mirrors `ui::history`'s tests.
    fn temp_store(tag: &str) -> NoteStore {
        let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}.json"));
        let _ = std::fs::remove_file(&path);
        NoteStore { notes: Vec::new(), path, dirty: false }
    }

    #[test]
    fn create_returns_a_unique_id_and_seeds_body_from_raw() {
        let mut store = temp_store("create");
        let a = store.create("call the vet".into(), NoteColor::Purple);
        let b = store.create("send invoice".into(), NoteColor::Teal);

        assert_ne!(a, b, "ids must be unique even within the same millisecond");

        let note = store.get(&a).unwrap();
        assert_eq!(note.raw, "call the vet");
        assert_eq!(note.body, "call the vet", "body starts as a copy of raw");
        assert_eq!(note.state, NoteState::Raw);
        assert!(!note.archived);
    }

    #[test]
    fn set_body_never_touches_raw() {
        let mut store = temp_store("raw_immutable");
        let id = store.create("um so call the vet".into(), NoteColor::Purple);

        store.set_body(&id, "Call the vet.".into());

        let note = store.get(&id).unwrap();
        assert_eq!(note.body, "Call the vet.");
        assert_eq!(
            note.raw, "um so call the vet",
            "raw is the only record of what was actually said and must survive cleanup"
        );
    }

    #[test]
    fn archive_hides_from_active_but_retains_the_note() {
        let mut store = temp_store("archive");
        let keep = store.create("keep".into(), NoteColor::Purple);
        let gone = store.create("archive me".into(), NoteColor::Rose);

        store.archive(&gone);

        let active: Vec<&str> = store.active().iter().map(|n| n.raw.as_str()).collect();
        assert_eq!(active, vec!["keep"]);
        assert!(store.get(&gone).is_some(), "archiving must not delete");
        let _ = keep;
    }

    #[test]
    fn flush_writes_only_when_dirty() {
        let mut store = temp_store("debounce");
        store.create("something".into(), NoteColor::Purple);

        assert!(store.flush_if_dirty(), "a pending change must be written");
        assert!(store.path.exists());
        assert!(
            !store.flush_if_dirty(),
            "a second flush with no intervening edit must not rewrite the file"
        );
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let mut store = temp_store("atomic");
        store.create("hello".into(), NoteColor::Purple);
        store.flush_if_dirty();

        assert!(!store.path.with_extension("json.tmp").exists());
    }

    #[test]
    fn notes_round_trip_through_disk() {
        let mut store = temp_store("roundtrip");
        let id = store.create("first".into(), NoteColor::Amber);
        store.set_geometry(&id, (100, 200), (320, 240));
        store.flush_if_dirty();

        let text = std::fs::read_to_string(&store.path).unwrap();
        let reloaded: NoteStore = serde_json::from_str(&text).unwrap();

        assert_eq!(reloaded.notes.len(), 1);
        assert_eq!(reloaded.notes[0].pos, Some((100, 200)));
        assert_eq!(reloaded.notes[0].size, Some((320, 240)));
        assert_eq!(reloaded.notes[0].color, NoteColor::Amber);
    }

    #[test]
    fn unknown_color_name_falls_back_to_purple() {
        assert_eq!(NoteColor::from_config_name("teal"), NoteColor::Teal);
        assert_eq!(
            NoteColor::from_config_name("chartreuse"), NoteColor::Purple,
            "a bad config value must not panic or produce an unrenderable color"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer notes:: 2>&1 | tail -20`
Expected: FAIL — nothing in `super` is defined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/notes/mod.rs`:

```rust
//! Sticky note storage.
//!
//! Follows `ui::history`'s load/save/corrupt-backup pattern, with two
//! deliberate differences: writes are debounced rather than per-change
//! (notes are edited per keystroke, and history's rewrite-everything-on-append
//! would be pathological here), and there is no entry cap — notes are authored
//! content, so they are archived rather than evicted.

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteState {
    Raw,
    Cleaned,
    CleanFailed,
    Analyzed,
    ExtractFailed,
}

/// Fixed palette rather than free-form hex: keeps notes inside the Deploy
/// Purple design language and keeps `notes.json` validatable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteColor {
    Purple,
    Violet,
    Amber,
    Teal,
    Rose,
    Slate,
}

impl NoteColor {
    pub fn from_config_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "violet" => Self::Violet,
            "amber" => Self::Amber,
            "teal" => Self::Teal,
            "rose" => Self::Rose,
            "slate" => Self::Slate,
            _ => Self::Purple,
        }
    }

    /// CSS class suffix used by `ui::sticky`.
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Purple => "purple",
            Self::Violet => "violet",
            Self::Amber => "amber",
            Self::Teal => "teal",
            Self::Rose => "rose",
            Self::Slate => "slate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub created: String,
    pub modified: String,
    /// Verbatim transcript or typed text. Never rewritten.
    pub raw: String,
    /// Display text. Equals `raw` until a cleanup pass replaces it.
    pub body: String,
    pub state: NoteState,
    pub color: NoteColor,
    /// Honored natively on Windows; on Wayland applied by the GNOME extension.
    pub pos: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub open: bool,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteStore {
    pub notes: Vec<Note>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
    #[serde(skip)]
    pub(crate) dirty: bool,
}

impl Default for NoteStore {
    fn default() -> Self {
        Self { notes: Vec::new(), path: Self::storage_path(), dirty: false }
    }
}

/// Monotonic within a process run, so two notes created in the same
/// millisecond still get distinct ids without pulling in a uuid dependency.
fn next_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let millis = Local::now().timestamp_millis();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{millis:x}-{n:04x}")
}

impl NoteStore {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("notes.json")
    }

    pub fn load() -> Self {
        let path = Self::storage_path();
        if !path.exists() {
            return Self::default();
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read notes at {:?}: {}", path, e);
                return Self::default();
            }
        };
        match serde_json::from_str::<NoteStore>(&contents) {
            Ok(mut store) => {
                store.path = path;
                store.dirty = false;
                store
            }
            Err(e) => {
                // Same reasoning as history.rs: don't start empty, or the next
                // write destroys the user's notes for good.
                let backup = path.with_extension("json.corrupt");
                tracing::error!(
                    "Notes at {:?} are not valid JSON ({}); preserving as {:?}",
                    path, e, backup
                );
                let _ = std::fs::rename(&path, &backup);
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Config::config_dir();
        std::fs::create_dir_all(&dir)?;
        let contents = serde_json::to_string_pretty(self)?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, contents)?;
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Write only if something changed since the last flush. Driven by a
    /// ~500ms interval task so per-keystroke edits coalesce into one write.
    pub fn flush_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        if let Err(e) = self.save() {
            tracing::error!("Failed to save notes: {}", e);
            // Stay dirty so the next tick retries rather than losing the edit.
            return false;
        }
        self.dirty = false;
        true
    }

    pub fn create(&mut self, raw: String, color: NoteColor) -> String {
        let now = Local::now().to_rfc3339();
        let id = next_id();
        self.notes.push(Note {
            id: id.clone(),
            created: now.clone(),
            modified: now,
            body: raw.clone(),
            raw,
            state: NoteState::Raw,
            color,
            pos: None,
            size: None,
            open: true,
            archived: false,
        });
        self.dirty = true;
        id
    }

    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    fn touch(&mut self, id: &str) -> Option<&mut Note> {
        let now = Local::now().to_rfc3339();
        let note = self.notes.iter_mut().find(|n| n.id == id)?;
        note.modified = now;
        Some(note)
    }

    pub fn set_body(&mut self, id: &str, body: String) {
        if let Some(note) = self.touch(id) {
            note.body = body;
            self.dirty = true;
        }
    }

    pub fn set_geometry(&mut self, id: &str, pos: (i32, i32), size: (u32, u32)) {
        if let Some(note) = self.touch(id) {
            note.pos = Some(pos);
            note.size = Some(size);
            self.dirty = true;
        }
    }

    pub fn set_open(&mut self, id: &str, open: bool) {
        if let Some(note) = self.touch(id) {
            note.open = open;
            self.dirty = true;
        }
    }

    pub fn archive(&mut self, id: &str) {
        if let Some(note) = self.touch(id) {
            note.archived = true;
            note.open = false;
            self.dirty = true;
        }
    }

    /// Non-archived notes, newest first.
    pub fn active(&self) -> Vec<&Note> {
        let mut v: Vec<&Note> = self.notes.iter().filter(|n| !n.archived).collect();
        v.sort_by(|a, b| b.created.cmp(&a.created));
        v
    }
}
```

Add `mod notes;` to `src/main.rs` beside the other module declarations.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --bin beamer notes:: 2>&1 | tail -20`
Expected: PASS, all seven tests.

- [ ] **Step 5: Commit**

```bash
git add src/notes/mod.rs src/main.rs
git commit -m "feat(notes): note store with debounced atomic persistence"
```

---

### Task 7: Orchestrator note sink

`src/orchestrator/mod.rs` is at 438 of the 500-line limit, so the new sink lives in its own file beside `session.rs`.

**Files:**
- Create: `src/orchestrator/sink.rs`
- Modify: `src/orchestrator/mod.rs`

**Interfaces:**
- Consumes: `CaptureMode` (Task 2), `NoteStore` / `NoteColor` (Task 6), `Config` (Task 5).
- Produces: `pub(super) async fn do_note_capture(text: &str, notes: &mut Signal<NoteStore>, config: &Signal<Config>, status_log: &mut Signal<StatusLog>) -> Option<String>` — returns the new note's id, or `None` when the transcript is blank.

- [ ] **Step 1: Write the failing test**

Create `src/orchestrator/sink.rs` with only its tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_transcripts_do_not_create_notes() {
        assert!(!should_create_note(""));
        assert!(!should_create_note("   "));
        assert!(!should_create_note("\n\t "));
        assert!(
            should_create_note("call the vet"),
            "real speech must always produce a note"
        );
    }

    #[test]
    fn note_mode_routes_away_from_injection() {
        assert!(!sink_injects(CaptureMode::Note));
        assert!(sink_injects(CaptureMode::Inject));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer orchestrator::sink 2>&1 | tail -20`
Expected: FAIL — `should_create_note` and `sink_injects` undefined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/orchestrator/sink.rs`:

```rust
//! Terminal sinks for a finished transcript.
//!
//! Split out of `mod.rs` to keep that file under the 500-line limit. The audio,
//! VAD, backend and tail-capture paths are shared by both capture modes; only
//! what happens to the final text differs.

use dioxus::prelude::*;

use crate::config::Config;
use crate::hotkey::CaptureMode;
use crate::notes::{NoteColor, NoteStore};
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Whether a transcript is worth persisting. A recording that produced only
/// silence should not leave an empty sticky note on the desktop.
pub(super) fn should_create_note(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Whether this mode's transcript goes to the injection chain.
pub(super) fn sink_injects(mode: CaptureMode) -> bool {
    matches!(mode, CaptureMode::Inject)
}

/// Create a sticky note from a finished transcript.
///
/// Returns the new note's id so the caller can open its window, or `None` when
/// there was nothing worth keeping.
pub(super) async fn do_note_capture(
    text: &str,
    notes: &mut Signal<NoteStore>,
    config: &Signal<Config>,
    status_log: &mut Signal<StatusLog>,
) -> Option<String> {
    if !should_create_note(text) {
        log_status(status_log, LogLevel::Info, "Nothing captured — no note created");
        return None;
    }

    let color = NoteColor::from_config_name(&config.peek().notes.default_color);
    let id = notes.write().create(text.to_string(), color);

    log_status(
        status_log,
        LogLevel::Info,
        format!("Note created ({} chars)", text.trim().len()),
    );
    Some(id)
}
```

In `src/orchestrator/mod.rs`:

1. Add `mod sink;` beside `mod session;`.
2. Add `notes: Signal<NoteStore>` and `capture_mode: CaptureMode` parameters through `run_orchestrator`, `handle_recording` and `handle_batch_recording`. `capture_mode` comes from the `HotkeyEvent::RecordStart(mode)` payload — replace the `RecordStart(_)` placeholder from Task 2 with `RecordStart(mode)` and thread it down.
3. At each of the three `TranscriptKind::Final` sites, branch:

```rust
if sink::sink_injects(capture_mode) {
    do_injection(&ev.text, &backends, &paste_shortcut, last_injection, history, status_log).await;
} else if let Some(id) = sink::do_note_capture(&ev.text, notes, config, status_log).await {
    // Task 8 replaces this with a real window open.
    tracing::info!("note {} created; window opening lands in Task 8", id);
}
```

Leave the `tracing::info!` exactly as written. Task 8 defines `open_note_window(window, registry, note)` and swaps it in — do not invent a call signature here, or the two tasks will disagree.

Update the `run_orchestrator` call site in `src/ui/app.rs` to pass the new `notes` signal, created with `use_signal(NoteStore::load)`.

- [ ] **Step 4: Run tests and build**

Run: `cargo test --bin beamer orchestrator::sink 2>&1 | tail -20`
Expected: PASS, both tests.

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 5: Verify end to end without any UI**

Set a note hotkey in `config.toml` (`note_hotkey = "Ctrl+Shift+N"`), then:

```bash
RUST_LOG=beamer=debug cargo run 2>&1 | grep -i "note\|RecordStart"
```

Press the note hotkey, speak, press again. Expected: `Hotkey triggered: RecordStart(Note)` then `Note created (N chars)`.

Confirm it persisted:

```bash
cat ~/.config/beamer/notes.json  # or the platform config dir
```

Expected: one note with your transcript in both `raw` and `body`.

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator/sink.rs src/orchestrator/mod.rs src/ui/app.rs
git commit -m "feat(orchestrator): route note-mode transcripts to the note store"
```

---

### Task 8: Sticky note window

**Files:**
- Create: `src/ui/sticky.rs`
- Modify: `src/ui/mod.rs` (add `pub mod sticky;`)
- Modify: `src/ui/app_setup.rs` (window registry + open/close plumbing)

**Interfaces:**
- Consumes: `NoteStore`, `Note`, `NoteColor` (Task 6).
- Produces:
  - `pub fn StickyNote(id: String) -> Element` — the note window's root component
  - `pub const STICKY_CSS: &str`
  - `pub fn window_title(id: &str) -> String` → `format!("Beamer Note {id}")`
  - `pub(super) async fn open_note_window(window: DesktopContext, registry: Signal<HashMap<String, DesktopContext>>, note: Note, config: Config)`
  - `pub(super) fn close_note_window(registry: Signal<HashMap<String, DesktopContext>>, id: &str)`

- [ ] **Step 1: Write the failing test**

Window rendering cannot be unit tested, but the title contract can — the GNOME extension matches on it, so a change here silently breaks placement.

Create `src/ui/sticky.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_title_embeds_the_note_id() {
        let title = window_title("18f2a1b3c4d-0001");
        assert_eq!(title, "Beamer Note 18f2a1b3c4d-0001");
        assert!(
            title.starts_with(TITLE_PREFIX),
            "the GNOME extension matches on this prefix; changing it breaks placement"
        );
    }

    #[test]
    fn titles_are_unique_per_note() {
        assert_ne!(window_title("a"), window_title("b"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer ui::sticky 2>&1 | tail -20`
Expected: FAIL — `window_title` and `TITLE_PREFIX` undefined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/ui/sticky.rs`:

```rust
//! Sticky note windows.
//!
//! One ordinary Dioxus window per open note — the same `new_window` pattern
//! used for the splash and pill windows in `app_setup.rs`. Deliberately NOT
//! always-on-top: notes sit in the normal stacking order.
//!
//! Wayland gives clients no control over their own position, so placement and
//! geometry read-back both go through Beamer's GNOME extension (`shell_window`).
//! The window title is the handle the extension matches on.

use dioxus::prelude::*;

use crate::notes::{NoteColor, NoteStore};

pub const TITLE_PREFIX: &str = "Beamer Note ";

pub fn window_title(id: &str) -> String {
    format!("{TITLE_PREFIX}{id}")
}

#[component]
pub fn StickyNote(id: String) -> Element {
    let mut notes = use_context::<Signal<NoteStore>>();

    let note = use_memo({
        let id = id.clone();
        move || notes.read().get(&id).cloned()
    });

    let Some(note) = note() else {
        // The note was archived from another window while this one was open.
        return rsx! { div { class: "sticky-gone", "This note was deleted." } };
    };

    let body = note.body.clone();
    let color_class = format!("sticky sticky-{}", note.color.css_class());
    let edit_id = id.clone();
    let archive_id = id.clone();

    rsx! {
        div { class: "{color_class}",
            div { class: "sticky-bar",
                div { class: "sticky-dots",
                    for c in [NoteColor::Purple, NoteColor::Violet, NoteColor::Amber,
                              NoteColor::Teal, NoteColor::Rose, NoteColor::Slate] {
                        button {
                            class: "sticky-dot sticky-dot-{c.css_class()}",
                            onclick: {
                                let id = id.clone();
                                move |_| { notes.write().set_color(&id, c); }
                            },
                        }
                    }
                }
                button {
                    class: "sticky-archive",
                    onclick: move |_| { notes.write().archive(&archive_id); },
                    "×"
                }
            }
            textarea {
                class: "sticky-body",
                value: "{body}",
                oninput: move |e| { notes.write().set_body(&edit_id, e.value()); },
            }
        }
    }
}
```

`set_color` is not yet on `NoteStore` — add it beside `set_body` in `src/notes/mod.rs`:

```rust
    pub fn set_color(&mut self, id: &str, color: NoteColor) {
        if let Some(note) = self.touch(id) {
            note.color = color;
            self.dirty = true;
        }
    }
```

Add the stylesheet in the same file, following Deploy Purple (2px borders, hard-offset shadow, solid backgrounds):

```rust
pub const STICKY_CSS: &str = r#"
*, *::before, *::after { margin:0; padding:0; box-sizing:border-box; }
html, body, #main { height:100%; overflow:hidden;
  font-family:"DM Mono","Segoe UI Variable","Segoe UI",monospace,system-ui,sans-serif; }

.sticky { display:flex; flex-direction:column; height:100%;
  border:2px solid #1a1a1f; box-shadow:4px 4px 0 #1a1a1f; }
.sticky-purple { background:#EDE4FB; } .sticky-violet { background:#E4E6FB; }
.sticky-amber  { background:#FBF1DC; } .sticky-teal   { background:#DCF5F0; }
.sticky-rose   { background:#FBE1E8; } .sticky-slate  { background:#E7E9EC; }

.sticky-bar { display:flex; align-items:center; justify-content:space-between;
  padding:6px 8px; border-bottom:2px solid #1a1a1f; }
.sticky-dots { display:flex; gap:5px; }
.sticky-dot { width:12px; height:12px; border:1.5px solid #1a1a1f; border-radius:50%;
  cursor:pointer; padding:0; }
.sticky-dot-purple{background:#8921E4} .sticky-dot-violet{background:#6B6BE4}
.sticky-dot-amber {background:#E4A421} .sticky-dot-teal  {background:#21C9B0}
.sticky-dot-rose  {background:#E4216B} .sticky-dot-slate {background:#8A93A0}

.sticky-archive { background:none; border:none; cursor:pointer;
  font-size:18px; line-height:1; color:#1a1a1f; padding:0 4px; }

.sticky-body { flex:1; width:100%; resize:none; border:none; outline:none;
  background:transparent; padding:10px; font:inherit; font-size:14px;
  line-height:1.5; color:#1a1a1f; }

.sticky-gone { padding:16px; font-size:13px; color:#6b6b73; }
"#;
```

In `src/ui/app_setup.rs`, add the registry and the open/close helpers, mirroring `setup_recording_pill`'s use of `new_window`:

```rust
use std::collections::HashMap;
use dioxus::desktop::tao::dpi::{LogicalPosition, LogicalSize};
use crate::notes::NoteStore;
use crate::ui::sticky::{window_title, StickyNote, StickyNoteProps, STICKY_CSS};

/// Maps note id → its live window, so a note cannot be opened twice and can be
/// closed programmatically when archived.
pub(super) type StickyRegistry = Signal<HashMap<String, DesktopContext>>;

pub(super) async fn open_note_window(
    window: DesktopContext,
    mut registry: StickyRegistry,
    note: crate::notes::Note,
) {
    if registry.peek().contains_key(&note.id) {
        return; // already open
    }

    let (w, h) = note.size.unwrap_or((320, 260));
    let mut builder = WindowBuilder::new()
        .with_title(window_title(&note.id))
        .with_decorations(false)
        .with_always_on_top(false)
        .with_inner_size(LogicalSize::new(w as f64, h as f64));

    // Honored on Windows; ignored by Mutter, which is why `shell_window`
    // exists. Set anyway so the Windows build needs no special case.
    if let Some((x, y)) = note.pos {
        builder = builder.with_position(LogicalPosition::new(x as f64, y as f64));
    }

    let id_for_dom = note.id.clone();
    let dom = VirtualDom::new_with_props(StickyNote, StickyNoteProps { id: id_for_dom });
    let cfg = DesktopConfig::new()
        .with_window(builder)
        .with_custom_head(format!("<style>{STICKY_CSS}</style>"));

    let ctx: DesktopContext = window.new_window(dom, cfg).await;
    registry.write().insert(note.id.clone(), ctx);
}

pub(super) fn close_note_window(mut registry: StickyRegistry, id: &str) {
    if let Some(ctx) = registry.write().remove(id) {
        ctx.close();
    }
}
```

Provide the `NoteStore` signal via context in `App()` so sticky windows can reach it:

```rust
use_context_provider(|| notes);
```

Replace the Task 7 stub call with the real `open_note_window`.

Add `pub mod sticky;` to `src/ui/mod.rs`.

- [ ] **Step 4: Run tests and build**

Run: `cargo test --bin beamer ui::sticky 2>&1 | tail -20`
Expected: PASS, both tests.

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 5: Verify a note window actually appears**

Run: `cargo run`

Press the note hotkey, speak, press again. Expected: a coloured sticky window appears with your transcript in it. Type into it, close Beamer, restart, and confirm `notes.json` holds the edited body.

Note: position will be wherever Mutter chose. Task 9 fixes that.

- [ ] **Step 6: Commit**

```bash
git add src/ui/sticky.rs src/ui/mod.rs src/ui/app_setup.rs src/notes/mod.rs
git commit -m "feat(ui): sticky note windows with colour palette and live editing"
```

---

### Task 9: GNOME extension — `PlaceWindow` and `GetWindowFrame`

Wayland forbids a client from both setting *and reading* its own window position. Beamer's own shell extension can do both, because it runs inside Mutter.

**Files:**
- Modify: `extension/beamer-focus@beamer.app/extension.js`
- Modify: `extension/beamer-focus@beamer.app/metadata.json`

**Interfaces:**
- Produces, on `app.beamer.FocusProvider`:
  - `PlaceWindow(s title, i x, i y, b all_workspaces) -> b ok`
  - `GetWindowFrame(s title) -> (b ok, i x, i y, u w, u h)`
  - `GetVersion()` now returns `5`

- [ ] **Step 1: Bump the version and declare the methods**

In `extension.js`, add to `DBUS_XML` inside `<interface name="app.beamer.FocusProvider">`:

```xml
    <method name="PlaceWindow">
      <arg type="s" direction="in"  name="title"/>
      <arg type="i" direction="in"  name="x"/>
      <arg type="i" direction="in"  name="y"/>
      <arg type="b" direction="in"  name="all_workspaces"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
    <method name="GetWindowFrame">
      <arg type="s" direction="in"  name="title"/>
      <arg type="b" direction="out" name="ok"/>
      <arg type="i" direction="out" name="x"/>
      <arg type="i" direction="out" name="y"/>
      <arg type="u" direction="out" name="w"/>
      <arg type="u" direction="out" name="h"/>
    </method>
```

Set `HELPER_VERSION = 5`, and set `"version": 5` in `metadata.json`.

- [ ] **Step 2: Implement the methods**

Add beside `ShowIndicator` in the same class:

```js
    // ── Sticky note window placement ─────────────────────────────────────────
    //
    // Wayland gives clients no control over their own geometry, and Mutter does
    // not implement wlr-layer-shell. Running inside the shell, we can do what
    // the client cannot. Windows are matched by exact title, which Beamer sets
    // to "Beamer Note <id>".

    _findWindowByTitle(title) {
        for (const actor of global.get_window_actors()) {
            const win = actor.meta_window;
            if (win && win.get_title() === title)
                return win;
        }
        return null;
    }

    PlaceWindow(title, x, y, allWorkspaces) {
        const win = this._findWindowByTitle(title);
        if (!win) return false;
        // user_op = true so Mutter treats this as a deliberate placement and
        // does not later re-position the window itself.
        win.move_frame(true, x, y);
        if (allWorkspaces) win.stick();
        return true;
    }

    GetWindowFrame(title) {
        const win = this._findWindowByTitle(title);
        if (!win) return [false, 0, 0, 0, 0];
        const r = win.get_frame_rect();
        return [true, r.x, r.y, r.width, r.height];
    }
```

- [ ] **Step 3: Install the updated extension and log out**

```bash
cp -r extension/beamer-focus@beamer.app \
  ~/.local/share/gnome-shell/extensions/
```

Then **log out and back in.** GNOME extensions do not hot-reload on Wayland; skipping this means testing the old code and drawing false conclusions.

- [ ] **Step 4: Verify the D-Bus surface by hand before writing Rust against it**

```bash
gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.GetVersion
```

Expected: `(uint32 5,)`. If it returns 4, the log-out did not take effect.

Then, with a sticky note window open (from Task 8), find its title and place it:

```bash
gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.PlaceWindow \
  "Beamer Note <paste-real-id>" 400 300 true
```

Expected: `(true,)` **and the window visibly jumps to 400,300.** A `(true,)` with no movement means `move_frame` is being ignored — investigate before proceeding.

```bash
gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.GetWindowFrame \
  "Beamer Note <paste-real-id>"
```

Expected: `(true, 400, 300, <w>, <h>)`.

- [ ] **Step 5: Commit**

```bash
git add extension/beamer-focus@beamer.app/
git commit -m "feat(extension): PlaceWindow and GetWindowFrame for sticky notes (v5)"
```

---

### Task 10: Rust client for window placement

**Files:**
- Create: `src/ui/shell_window.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/app_setup.rs`

**Interfaces:**
- Consumes: extension v5 methods (Task 9), `window_title` (Task 8).
- Produces:
  - `pub fn place(title: String, x: i32, y: i32, all_workspaces: bool)` — fire-and-forget, retries internally
  - `pub fn frame(title: &str) -> Option<(i32, i32, u32, u32)>` — blocking; call from `spawn_blocking`

- [ ] **Step 1: Write the failing test**

Create `src/ui/shell_window.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_schedule_is_bounded_and_covers_about_two_seconds() {
        let delays = retry_delays();
        assert!(!delays.is_empty(), "at least one retry is required");
        let total: u64 = delays.iter().map(|d| d.as_millis() as u64).sum();
        assert!(
            (1500..=2500).contains(&total),
            "retries should span roughly 2s, got {total}ms — the window may not \
             exist from Mutter's point of view the instant new_window returns"
        );
        assert!(
            delays.windows(2).all(|w| w[1] >= w[0]),
            "delays must be non-decreasing (backoff, not thrash)"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer ui::shell_window 2>&1 | tail -20`
Expected: FAIL — `retry_delays` undefined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/ui/shell_window.rs`:

```rust
#![cfg(target_os = "linux")]

//! Client for the GNOME extension's sticky-note window placement (v5).
//!
//! Separate from `shell_indicator`'s worker on purpose: that one is a single
//! long-lived thread with a 200ms timeout tuned for ~15Hz level updates during
//! recording, and blocking it for two seconds of placement retries would stall
//! the pill's waveform. Placement is rare, so it gets its own short-lived
//! thread per call.

use std::time::Duration;

const DBUS_DEST: &str = "org.gnome.Shell";
const DBUS_PATH: &str = "/app/beamer/FocusProvider";
const DBUS_IFACE: &str = "app.beamer.FocusProvider";

/// Backoff schedule for `PlaceWindow`.
///
/// Beamer cannot observe the Wayland map event, so the window may not exist
/// from Mutter's point of view when `new_window()` returns. Retry across
/// roughly two seconds, then give up — an unplaced note is a cosmetic loss,
/// not a failure worth logging loudly.
fn retry_delays() -> Vec<Duration> {
    vec![
        Duration::from_millis(80),
        Duration::from_millis(120),
        Duration::from_millis(200),
        Duration::from_millis(300),
        Duration::from_millis(450),
        Duration::from_millis(700),
    ]
}

fn proxy() -> Option<zbus::blocking::Proxy<'static>> {
    let conn = zbus::blocking::Connection::session().ok()?;
    zbus::blocking::Proxy::new(&conn, DBUS_DEST, DBUS_PATH, DBUS_IFACE).ok()
}

/// Ask the shell to move a note window. Fire and forget.
pub fn place(title: String, x: i32, y: i32, all_workspaces: bool) {
    std::thread::Builder::new()
        .name("beamer-place-window".into())
        .spawn(move || {
            let Some(proxy) = proxy() else { return };
            for delay in retry_delays() {
                std::thread::sleep(delay);
                let ok: Result<bool, _> =
                    proxy.call("PlaceWindow", &(title.as_str(), x, y, all_workspaces));
                match ok {
                    Ok(true) => {
                        tracing::debug!("Placed {} at {},{}", title, x, y);
                        return;
                    }
                    Ok(false) => continue, // window not mapped yet
                    Err(e) => {
                        // No helper extension, or it's older than v5.
                        tracing::debug!("PlaceWindow unavailable: {}", e);
                        return;
                    }
                }
            }
            tracing::debug!("Gave up placing {}; leaving it where Mutter put it", title);
        })
        .ok();
}

/// Read a note window's current frame. Blocking — call inside `spawn_blocking`.
pub fn frame(title: &str) -> Option<(i32, i32, u32, u32)> {
    let proxy = proxy()?;
    let (ok, x, y, w, h): (bool, i32, i32, u32, u32) =
        proxy.call("GetWindowFrame", &(title,)).ok()?;
    ok.then_some((x, y, w, h))
}
```

Add `pub mod shell_window;` to `src/ui/mod.rs` (Linux-gated by the file's own `#![cfg]`).

In `app_setup.rs`, call `place` right after `new_window` returns, and capture geometry on close:

```rust
    let ctx: DesktopContext = window.new_window(dom, cfg).await;
    registry.write().insert(note.id.clone(), ctx);

    #[cfg(target_os = "linux")]
    if let Some((x, y)) = note.pos {
        crate::ui::shell_window::place(window_title(&note.id), x, y, all_workspaces);
    } else if all_workspaces {
        // No saved position, but still make it follow across workspaces.
        crate::ui::shell_window::place(window_title(&note.id), -1, -1, true);
    }
```

For the no-saved-position case, guard in the extension: skip `move_frame` when `x < 0 && y < 0`, and only apply `stick()`. Add that guard to `PlaceWindow` in `extension.js`:

```js
        if (x >= 0 || y >= 0) win.move_frame(true, x, y);
```

Add geometry capture in `close_note_window`:

Replace the body of `close_note_window` from Task 8 (the signature is already correct) with:

```rust
pub(super) fn close_note_window(
    mut registry: StickyRegistry,
    mut notes: Signal<NoteStore>,
    id: &str,
) {
    #[cfg(target_os = "linux")]
    {
        let title = window_title(id);
        if let Some((x, y, w, h)) = crate::ui::shell_window::frame(&title) {
            notes.write().set_geometry(id, (x, y), (w, h));
        }
    }
    if let Some(ctx) = registry.write().remove(id) {
        ctx.close();
    }
    notes.write().set_open(id, false);
}
```

- [ ] **Step 4: Run tests and build**

Run: `cargo test --bin beamer ui::shell_window 2>&1 | tail -20`
Expected: PASS.

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 5: Verify placement persists across a restart**

Run `cargo run`, create a note, drag it somewhere distinctive, close the note window, quit Beamer.

```bash
grep -A2 '"pos"' ~/.config/beamer/notes.json
```

Expected: the dragged coordinates, not `null`.

Restart Beamer and reopen the note from the notes board (Task 11). Expected: **it reappears where you left it.** This is the payoff for the whole extension detour — without it, Wayland notes wander.

- [ ] **Step 6: Commit**

```bash
git add src/ui/shell_window.rs src/ui/mod.rs src/ui/app_setup.rs extension/beamer-focus@beamer.app/extension.js
git commit -m "feat(ui): persist and restore sticky note geometry via the shell extension"
```

---

### Task 11: Notes board page

**Files:**
- Create: `src/ui/notes_page.rs`
- Modify: `src/ui/app.rs` (Page enum, sidebar, routing)
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/icons.rs` (note icon)

**Interfaces:**
- Consumes: `NoteStore`, `Note` (Task 6), `open_note_window` / `close_note_window` (Tasks 8, 10).
- Produces: `pub fn NotesPage() -> Element`.

- [ ] **Step 1: Write the failing test**

Search behaviour is pure logic and belongs in the store, where it can be tested. Add to `src/notes/mod.rs`'s tests:

```rust
    #[test]
    fn search_matches_raw_and_body_case_insensitively() {
        let mut store = temp_store("search");
        let a = store.create("um call the VET about milo".into(), NoteColor::Purple);
        store.set_body(&a, "Call the vet about Milo.".into());
        store.create("send the invoice".into(), NoteColor::Teal);

        assert_eq!(store.search("vet").len(), 1);
        assert_eq!(store.search("VET").len(), 1, "search must be case-insensitive");
        assert_eq!(store.search("milo").len(), 1, "matching raw alone is enough");
        assert_eq!(store.search("").len(), 2, "an empty query returns everything active");
        assert_eq!(store.search("nonexistent").len(), 0);
    }

    #[test]
    fn search_excludes_archived_notes() {
        let mut store = temp_store("search_archived");
        let id = store.create("archived vet note".into(), NoteColor::Purple);
        store.archive(&id);
        assert_eq!(store.search("vet").len(), 0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin beamer notes:: 2>&1 | tail -20`
Expected: FAIL — no method `search`.

- [ ] **Step 3: Write minimal implementation**

Add to `NoteStore` in `src/notes/mod.rs`:

```rust
    /// Case-insensitive substring search over both `raw` and `body`, newest
    /// first. Searching `raw` too means a note still turns up under the words
    /// actually spoken, even after cleanup rephrased them.
    pub fn search(&self, query: &str) -> Vec<&Note> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.active();
        }
        let mut v: Vec<&Note> = self
            .notes
            .iter()
            .filter(|n| !n.archived)
            .filter(|n| n.raw.to_lowercase().contains(&q) || n.body.to_lowercase().contains(&q))
            .collect();
        v.sort_by(|a, b| b.created.cmp(&a.created));
        v
    }
```

Create `src/ui/notes_page.rs`:

```rust
//! All-notes board: a grid of note cards. Clicking one pops out its sticky
//! window. Modelled on vixalien/sticky's collection view.

use dioxus::prelude::*;

use crate::notes::NoteStore;
use crate::ui::components::Card;

#[component]
pub fn NotesPage() -> Element {
    let notes = use_context::<Signal<NoteStore>>();
    let mut query = use_signal(String::new);

    let results = use_memo(move || {
        notes
            .read()
            .search(&query.read())
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
    });

    rsx! {
        Card { title: "Notes".to_string(),
            input {
                class: "notes-search",
                placeholder: "Search notes…",
                value: "{query}",
                oninput: move |e| query.set(e.value()),
            }

            if results().is_empty() {
                div { class: "notes-empty",
                    "No notes yet. Press your note hotkey and start talking."
                }
            } else {
                div { class: "notes-grid",
                    for note in results() {
                        div {
                            key: "{note.id}",
                            class: "note-card note-card-{note.color.css_class()}",
                            onclick: {
                                let id = note.id.clone();
                                move |_| { super::app_setup::request_open_note(id.clone()); }
                            },
                            div { class: "note-card-body", "{note.body}" }
                            div { class: "note-card-meta", "{note.created}" }
                        }
                    }
                }
            }
        }
    }
}
```

`request_open_note` decouples the page from the async window plumbing. Add to `app_setup.rs`:

```rust
/// Notes the UI wants opened. Drained by an effect in `App()` that owns the
/// `DesktopContext` needed to actually create a window — a page component
/// can't call `new_window` itself.
pub(super) static PENDING_OPENS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn request_open_note(id: String) {
    if let Ok(mut q) = PENDING_OPENS.lock() {
        q.push(id);
    }
}

pub(super) fn setup_note_opener(
    window: DesktopContext,
    registry: StickyRegistry,
    notes: Signal<NoteStore>,
) {
    use_future(move || {
        let window = window.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                let pending: Vec<String> = {
                    let Ok(mut q) = PENDING_OPENS.lock() else { continue };
                    std::mem::take(&mut *q)
                };
                for id in pending {
                    let note = notes.peek().get(&id).cloned();
                    if let Some(note) = note {
                        open_note_window(window.clone(), registry, note).await;
                    }
                }
            }
        }
    });
}
```

Also add the debounced-save ticker here, which Task 6's store was built for:

```rust
pub(super) fn setup_note_autosave(mut notes: Signal<NoteStore>) {
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            notes.write().flush_if_dirty();
        }
    });
}
```

In `src/ui/app.rs`: add `Notes` to the `Page` enum, a sidebar button beside History using a note glyph from `icons.rs`, a `Page::Notes => rsx! { NotesPage {} }` arm, and calls to `setup_note_opener` and `setup_note_autosave` in `App()`'s body.

Add the grid styles to the main stylesheet:

```css
.notes-search { width:100%; padding:8px 10px; margin-bottom:12px;
  border:2px solid #1a1a1f; background:#fff; font:inherit; font-size:13px; }
.notes-grid { display:grid; gap:12px;
  grid-template-columns:repeat(auto-fill, minmax(180px, 1fr)); }
.note-card { border:2px solid #1a1a1f; box-shadow:3px 3px 0 #1a1a1f;
  padding:10px; cursor:pointer; min-height:110px;
  display:flex; flex-direction:column; justify-content:space-between; }
.note-card-purple{background:#EDE4FB} .note-card-violet{background:#E4E6FB}
.note-card-amber {background:#FBF1DC} .note-card-teal  {background:#DCF5F0}
.note-card-rose  {background:#FBE1E8} .note-card-slate {background:#E7E9EC}
.note-card-body { font-size:13px; line-height:1.45; color:#1a1a1f;
  overflow:hidden; display:-webkit-box; -webkit-line-clamp:5; -webkit-box-orient:vertical; }
.note-card-meta { font-size:10px; color:#6b6b73; margin-top:8px; }
.notes-empty { padding:24px; text-align:center; font-size:13px; color:#6b6b73; }
```

- [ ] **Step 4: Run tests and build**

Run: `cargo test --bin beamer notes:: 2>&1 | tail -20`
Expected: PASS, all nine tests.

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 5: Verify the full loop**

Run: `cargo run`

1. Press the note hotkey, dictate, press again → sticky window appears.
2. Open the Notes page → the note is on the board.
3. Close the sticky window, click the card → it reopens **in the same place**.
4. Search for a word you said → the card is found.
5. Quit and restart → notes and positions survive.

- [ ] **Step 6: Check the file-size constraint**

Run: `wc -l src/notes/mod.rs src/ui/sticky.rs src/ui/notes_page.rs src/ui/app_setup.rs src/ui/app.rs src/hotkey/linux_hotkey.rs`
Expected: every file under 500 lines. `app_setup.rs` is the likely offender — if it exceeds, split the sticky-window helpers into `src/ui/sticky_windows.rs` and commit that as part of this task.

- [ ] **Step 7: Commit**

```bash
git add src/ui/notes_page.rs src/ui/app.rs src/ui/mod.rs src/ui/app_setup.rs src/ui/icons.rs src/notes/mod.rs assets/
git commit -m "feat(ui): notes board page with search and reopen"
```

---

### Task 11b: LLM connection config and settings card

Added 2026-08-21. Beamer is now a plain HTTP client to a standalone llama.cpp
server (spec §7), so it needs a way to point at that server and confirm it is
reachable. This is deliberately its own task: it touches config and settings UI
only, and it ships useful on its own — before any cleanup pass exists, the card
tells you whether the server the later phases depend on is actually up.

**Files:**
- Create: `src/llm/mod.rs`, `src/llm/client.rs`
- Create: `src/ui/settings/local_ai_card.rs`
- Modify: `src/config/mod.rs` (add `LlmConfig`)
- Modify: `src/ui/settings/mod.rs` (register the card)
- Modify: `agent_docs/config_schema.md`

**Interfaces:**
- Produces:
  - `pub struct LlmConfig { enabled: bool, base_url: String, request_timeout_ms: u64, cleanup: CleanupConfig, extract: ExtractConfig }`
  - `pub async fn probe(base_url: &str, timeout: Duration) -> Result<Vec<ModelStatus>, ProbeError>`
  - `pub struct ModelStatus { pub id: String, pub state: String }`

- [ ] **Step 1: Write the failing tests**

In `src/llm/client.rs`, following the repo's inline `#[cfg(test)]` pattern:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_list_and_state() {
        let body = r#"{"data":[
            {"id":"s1-mini-q4_k_m","status":{"value":"loaded"}},
            {"id":"gemma-4-E4B_q4_0-it","status":{"value":"sleeping"}}
        ]}"#;
        let got = parse_models(body).expect("valid body must parse");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "s1-mini-q4_k_m");
        assert_eq!(got[1].state, "sleeping",
            "the card surfaces sleep state; it must survive parsing");
    }

    #[test]
    fn base_url_tolerates_a_trailing_slash() {
        assert_eq!(models_url("http://127.0.0.1:8080/"), "http://127.0.0.1:8080/v1/models");
        assert_eq!(models_url("http://127.0.0.1:8080"),  "http://127.0.0.1:8080/v1/models");
    }

    #[test]
    fn default_config_points_at_localhost_and_is_enabled() {
        let c = LlmConfig::default();
        assert_eq!(c.base_url, "http://127.0.0.1:8080");
        assert!(c.request_timeout_ms >= 15_000,
            "must cover a cold server start (~4 s) plus generation, with margin");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --bin beamer llm:: 2>&1 | tail -20`
Expected: FAIL — module `llm` does not exist.

- [ ] **Step 3: Implement the client and config**

`parse_models` reads `data[].id` and `data[].status.value`. `probe` issues a
single `GET {base_url}/v1/models` with the configured timeout.

**`probe` must be called on demand only — never on a timer.** Status reads reset
the server's per-model idle clock, so a periodic health check pins the ~3 GB
extraction model in VRAM permanently, with no error and no visible symptom
(spec §2, benchmark doc Finding 6). Put that warning in a doc comment on
`probe` itself, where the next person to add a "background health check" will
read it.

All blocking HTTP goes through `tokio::task::spawn_blocking`.

- [ ] **Step 4: Build the settings card**

`local_ai_card.rs`, following `transcription_card.rs` for structure and
`components.rs` for `Card` / `Select` / `Toggle`. Deploy Purple throughout.

- Enable toggle, server URL text input.
- **Test connection** button → `probe` → render either the model list with each
  model's state, or a plain failure ("Server not running" / status line / "No
  response").
- The cleanup and extraction model pickers are `Select`s populated from the last
  successful probe, falling back to free text seeded from config when no probe
  has succeeded. This stops the user typing a model name that must match a GGUF
  filename stem exactly.
- Probe once when the settings window opens, and on button press. Nowhere else.

- [ ] **Step 5: Verify**

Run: `cargo test --bin beamer llm:: 2>&1 | tail -20` → PASS
Run: `cargo build 2>&1 | tail -20` → clean

Manual: with the server up, the card lists both models and their states; stop it
with `pkill -x llama-server` and confirm the card reports it is not running and
says notes are still captured without cleanup.

- [ ] **Step 6: Commit**

```bash
git add src/llm/ src/ui/settings/local_ai_card.rs src/ui/settings/mod.rs src/config/mod.rs agent_docs/config_schema.md
git commit -m "feat(llm): connection config and Local AI settings card"
```

---

### Task 12: Documentation

**Files:**
- Create: `agent_docs/sticky_notes.md`
- Modify: `agent_docs/dioxus_architecture.md`
- Modify: `CLAUDE.md`
- Modify: `todo.md`

- [ ] **Step 1: Write `agent_docs/sticky_notes.md`**

Cover, with file references: the `CaptureMode` split and where the three `TranscriptKind::Final` branch points are; the note state machine and the rule that `raw` is never overwritten; the debounce mechanism and why it differs from `history.rs`; the extension v5 D-Bus contract with the exact title format `Beamer Note <id>`; and — prominently — that **GNOME extensions require a full log out to reload on Wayland**, since that will otherwise cost every future contributor an hour.

- [ ] **Step 2: Correct the stale claim in `dioxus_architecture.md`**

Its first paragraph currently says *"There is no multi-window overlay/glow system"* and lists only the main, pill and splash windows. Sticky notes make that wrong. Update the window inventory and describe the `StickyRegistry` pattern.

- [ ] **Step 3: Correct the spec's file-layout table**

`docs/superpowers/specs/2026-08-20-sticky-notes-design.md` §12 does not list `src/ui/shell_window.rs`, which this plan introduces because Wayland forbids reading a window's own geometry — a constraint the spec missed. Add it to the table so the spec and the tree agree.

- [ ] **Step 4: Update `CLAUDE.md`**

Add `src/notes/` to the project structure list, and add `agent_docs/sticky_notes.md` to the detailed-docs list.

- [ ] **Step 5: Update `todo.md`**

Record what Phase 1 deliberately left out: no cleanup pass, no task extraction, no Tasks page, no settings UI for the note hotkey (config-file only), and the Windows hotkey change is unverified.

- [ ] **Step 6: Commit**

```bash
git add agent_docs/ CLAUDE.md todo.md docs/superpowers/specs/
git commit -m "docs: sticky notes architecture and phase 1 scope"
```

---

### Task 13: Recording pill shows note mode

Spec §6 requires the pill to make it obvious which hotkey was hit. Today `linux_integration.rs` drives the pill from `rec_state` alone, which knows nothing about capture mode, so this needs one extra signal.

**Files:**
- Modify: `src/orchestrator/mod.rs`
- Modify: `src/ui/linux_integration.rs`
- Modify: `src/ui/app.rs`
- Modify: `extension/beamer-focus@beamer.app/indicator.js`
- Modify: `extension/beamer-focus@beamer.app/stylesheet.css`

**Interfaces:**
- Consumes: `CaptureMode` (Task 2).
- Produces: `active_mode: Signal<CaptureMode>` owned by `App()`, written by the orchestrator at session start; the extension accepts `"note"` as a `ShowIndicator` state.

- [ ] **Step 1: Add the signal and set it at session start**

In `src/ui/app.rs`:

```rust
let active_mode = use_signal(|| CaptureMode::Inject);
```

Pass it into `run_orchestrator` alongside `rec_state`. In `src/orchestrator/mod.rs`, where `HotkeyEvent::RecordStart(mode)` is handled and `rec_state.set(RecordingState::Recording)` happens, add:

```rust
active_mode.set(mode);
```

Set it **before** `rec_state`, so the effect in `linux_integration` reads the correct mode on the same render rather than one frame late.

- [ ] **Step 2: Choose the pill state from both signals**

In `src/ui/linux_integration.rs`, replace the state match inside the existing `use_effect`:

```rust
        let mode = *active_mode.read();
        match state {
            RecordingState::Recording if pill_enabled => {
                crate::ui::shell_indicator::show(match mode {
                    CaptureMode::Note => "note",
                    CaptureMode::Inject => "recording",
                })
            }
            RecordingState::Processing if pill_enabled => {
                crate::ui::shell_indicator::show("processing")
            }
            _ => crate::ui::shell_indicator::hide(),
        }
```

`shell_indicator::show` takes `&'static str`, so both arms are string literals — no allocation, no signature change.

- [ ] **Step 3: Teach the extension the new state**

In `indicator.js`, wherever the `show(state)` method applies a style class for `recording` / `processing`, add a `note` branch that applies a `beamer-pill-note` class. Keep the waveform animation — this is still recording, just to a different destination.

In `stylesheet.css`, add a variant that reads as clearly different at a glance:

```css
.beamer-pill-note {
  border-color: #8921E4;
  box-shadow: 0 0 0 2px rgba(137, 33, 228, 0.35), 0 6px 18px rgba(0,0,0,0.45);
}
```

Reinstall the extension and **log out and back in** — extensions do not hot-reload on Wayland.

```bash
cp -r extension/beamer-focus@beamer.app ~/.local/share/gnome-shell/extensions/
```

- [ ] **Step 4: Build and verify both pills**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

Run: `cargo run`

Press the **dictation** hotkey. Expected: the normal pill. Press the **note** hotkey. Expected: the same waveform with the purple ring. If both look identical, the extension did not reload — log out again.

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator/mod.rs src/ui/linux_integration.rs src/ui/app.rs extension/beamer-focus@beamer.app/
git commit -m "feat(ui): distinct recording pill state for note capture"
```

---

## Phase 1 done — what exists

Dictate with a second hotkey → a coloured sticky note appears on the GNOME desktop → edit it → it remembers its text, colour, size and position across restarts → find it again on a searchable board. No AI anywhere.

## What Phase 2 needs from Phase 1

- **Task 1's benchmark numbers.** Phase 2's plan is written against measured latency, not estimates. If S1-mini is slower than ~1.5 s per note, Phase 2 is a different design.
- **Real captured notes.** Phase 3's extraction model is chosen by measuring against a corpus of your actual dictation, which does not exist until Phase 1 has been in use for a while. Use Phase 1 for a week before Phase 3 is planned.

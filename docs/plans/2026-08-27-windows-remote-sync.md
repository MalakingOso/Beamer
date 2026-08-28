# Beamer on Windows: remote models, and sync that actually merges

## Context

Beamer is Windows-first by design (UIA injection, Credential Manager, a
low-level keyboard hook) but the last several months of work happened on
Linux/GNOME Wayland. Three things have to become true.

1. **The Windows target has to compile.** It does not. The note-hotkey feature
   landed on the Linux hook only, and `src/hotkey/ll_hook.rs` drifted from its
   callers in four places.
2. **The models cannot come with it.** llama.cpp runs on callisto's Arc Pro B60
   via SYCL; the laptop has no such GPU. Beamer is already a plain HTTP client
   whose entire connection surface is one config string, so it reaches callisto
   over the existing tailnet rather than shipping a second inference stack.
3. **Notes have to exist on both machines.** Today each machine is an island,
   and the storage layer is built on assumptions that make naive file sync
   destructive rather than merely lossy.

Verified while planning: the tailnet is live with MagicDNS on
(`callisto.taila63f23.ts.net`; `bearcave` online, directly connected, RTT
13-289 ms); callisto does not auto-suspend on AC; reqwest is built with
`native-tls` so an `https://` base_url validates against the OS trust store
with **zero code changes**.

Phase A (Tasks 1-6) ships a working Windows Beamer talking to callisto and
merges to master on its own. Phase B (Tasks 7-11) starts after, against a
Beamer you can already run on both machines.

## Global Constraints

These bind every task. A change that violates one is a defect regardless of
what its own task text says.

- **Every source file stays under 500 lines.** Split with `#[path]`-included
  `tests.rs` modules the way `task_store.rs`, `prompts.rs`, `extract.rs` and
  `tasks_page.rs` already do.
- **No crate-rooted paths (`crate::…`) anywhere under `src/llm/`.**
  `src/bin/task_eval.rs` `#[path]`-includes that directory and there is no
  `src/lib.rs`. Use `super::` / `self::` there.
- **Dioxus 0.7 owns the main thread and the tokio runtime.** Never create a
  second runtime. Inside Dioxus scopes use `spawn`, never `tokio::spawn` — the
  `Signal` generational-box arena is thread-local.
- **Never poll the model server on a timer.** `GET /v1/models` resets the
  server's per-model idle clock, so a background health check pins the ~3 GB
  extraction model in VRAM forever with no symptom. Probe on demand only.
- **`hook_proc` must stay cheap.** `LowLevelKeyboardProc` has a hard timeout
  capped at 1000 ms since Windows 10 1709 and a hook that exceeds it is
  silently removed with no way for the app to notice. `GetAsyncKeyState` plus a
  channel `send` is the entire budget: no logging, no COM, nothing blocking.
- **Prose style** for commit messages, doc comments and docs: no em dashes, no
  "not just X, it's Y" parallelism, no `**Bold term**:` bullet stacks as the
  default shape, no throat-clearing, no wrap-up paragraph. Contractions on.
  Banned words: delve, leverage (verb), robust, seamless, pivotal, crucial,
  comprehensive, transformative, tapestry, testament, underscore, realm,
  landscape, "it's worth noting", "in today's".
- **No Claude attribution in commits or code.** No `Co-Authored-By: Claude`,
  no "Generated with Claude Code", no "added with Claude".
- **Never mention IPEX** (`intel-extension-for-pytorch`). It is retired and
  past end of life.
- Build gate for Windows work: `XWIN_ACCEPT_LICENSE=1 cargo xwin check --target
  x86_64-pc-windows-msvc`. Linux gate: `cargo test`.
- Placeholder tokens (`[[beamer:<id>]]`, see `src/notes/blocks.rs`) must never
  reach a model.

---

## Task 1: Make the Windows hotkey layer compile, and give it a second binding

`src/hotkey/ll_hook.rs` has four defects:

| # | Where | What |
|---|---|---|
| 1 | `:137,143` | bare `HotkeyEvent::RecordStart`; it takes a `CaptureMode` now (E0308 x2) |
| 2 | `:205-208` | `start_ll_hook` takes 2 params, `ui/app.rs:114` passes 3 (E0061) |
| 3 | `:189` | `update_config`, `ui/app.rs:132` calls `update_configs` (E0599) |
| 4 | `:35-46` | four loose bools; **a second binding is unrepresentable** |

Defect 4 is the real work.

**Hoist, don't duplicate.** `src/hotkey/linux_hotkey.rs:20-72` already has the
platform-neutral matching layer, with four unit tests at `:396-479`. Move
these six items into `src/hotkey/mod.rs`, unchanged in behaviour:

- `MAX_BINDINGS` (currently `pub(super) const … = 2`)
- `BindingConfig { mode: CaptureMode, config: HotkeyConfig }`
- `BindingState { armed, toggled_on, trigger_held }`
- `Modifiers { ctrl, alt, shift }`
- `build_bindings(inject, note) -> Vec<BindingConfig>`
- `matching_binding(&[BindingConfig], vk, mods) -> Option<usize>`

Move the four tests that cover them (`chords_sharing_a_trigger_key_are_told_apart_by_modifiers_alone`,
`each_binding_matches_only_its_own_chord`, `bindings_keep_independent_press_state`,
`absent_note_binding_yields_only_one_binding`) into `mod.rs`'s test module
alongside them. They must keep passing verbatim. `linux_hotkey.rs` then imports
them from `super` and loses its own copies. Because both platforms now use the
items, they need to be visible to both: `pub(crate)` or `pub` in `mod.rs`, not
`pub(super)`.

If moving the tests pushes `src/hotkey/mod.rs` over 500 lines, split the test
module into `src/hotkey/tests.rs` reached by `#[path = "tests.rs"] mod tests;`,
matching how `task_store.rs` does it.

**Then rewrite `ll_hook.rs` around the shared layer.** The shape mirrors
`linux_hotkey.rs`'s `HookState`/`handle_key_event`:

- `HookState` holds `bindings: Arc<Mutex<Vec<BindingConfig>>>` and
  `binding_state: [BindingState; MAX_BINDINGS]` in place of the loose
  `config`/`armed`/`toggled_on`/`trigger_held` fields. Keep `ctrl_held`,
  `alt_held`, `shift_held` and `win_consumed`.
- `HotkeyHandle::update_config(HotkeyConfig)` becomes
  `update_configs(&self, inject: HotkeyConfig, note: Option<HotkeyConfig>)`,
  storing `build_bindings(inject, note)` and setting the reset flag. Keep the
  `Drop` impl that posts `WM_QUIT`.
- `start_ll_hook(inject: HotkeyConfig, note: Option<HotkeyConfig>, tx:
  UnboundedSender<HotkeyEvent>) -> HotkeyHandle` — the same signature
  `linux_hotkey.rs` already exposes and `ui/app.rs:114` already calls.
- The reset flag clears **every** binding:
  `state.binding_state = [BindingState::default(); MAX_BINDINGS]`, plus
  `win_consumed = false`.
- Both `RecordStart` sites send `HotkeyEvent::RecordStart(mode)` where `mode`
  is the matched binding's `mode`.

Five behaviours the port must preserve. Each is Windows-specific or already
load-bearing, and each is where this port goes subtly wrong:

1. **Release matches on held state, not on the chord.** On press, use
   `matching_binding(&bindings, vk, mods)`. On release the modifiers may
   already be up, so re-matching the chord finds nothing and strands the
   binding in `armed`. Instead find the binding whose `trigger_vk` equals this
   key *and* whose `binding_state[i].trigger_held` is true — the same rule
   `linux_hotkey.rs:handle_key_event` uses.
2. **The first-press physical modifier check stays.** Windows can drop key-up
   events, so on a first press the hook reads `GetAsyncKeyState` for both
   left and right Ctrl/Alt/Shift, resyncs `state.ctrl_held`/`alt_held`/
   `shift_held` from that reality, and matches with those values. Do this
   **once per press**, before calling `matching_binding`, and pass the
   resynced values in as the `Modifiers`. There is no Linux counterpart.
3. **The Win-key trigger still matches either side.** A binding whose
   `trigger_vk == VK_LWIN` is triggered by `VK_LWIN` (0x5B) or `VK_RWIN`
   (0x5C). Normalise the incoming `vk` to `VK_LWIN` before matching so a
   right-Win press matches, exactly as `linux_hotkey.rs` normalises
   `KEY_RIGHTMETA`.
4. **`win_consumed` suppression stays.** Set it when the binding that matched
   has a `VK_LWIN` trigger, and while set, suppress every Win press (including
   repeats) by returning `LRESULT(1)`; clear and suppress on release. One flag
   still suffices because at most one binding matches a given press.
5. **`hook_proc` stays cheap** — see Global Constraints. No `tracing` calls
   inside it; `linux_hotkey.rs` logs there but its hook has no timeout.

**Close the Super footgun in the same task.** `HotkeyConfig` has no Meta
field, so `HotkeyConfig::parse("Super+N", …)` currently returns
`ctrl/alt/shift = false, trigger_vk = 'N'` — the Super is silently dropped and
the binding fires on a **bare N keypress**. Take the fail-safe fix in
`src/hotkey/mod.rs`: `parse()` returns `None` when a Super/Win/Cmd/Meta token
appears *together with* another key, because `note_hotkey_config()` already
reads `None` as unbound. `Super` alone as the trigger (`has_win &&
key_str.is_empty()`, giving `VK_LWIN`) must keep working — `Ctrl+Super` and
`Ctrl+Alt+Super` are the currently-safe pairing and a test pins them.

**Tests to add** (Linux-runnable, in `src/hotkey/mod.rs`'s test module):

- `super_with_another_key_is_rejected`: `parse("Super+N", false).is_none()`,
  and likewise `"Ctrl+Super+N"`.
- `super_alone_still_parses_as_the_trigger`: `parse("Ctrl+Super", false)`
  yields `ctrl = true, trigger_vk == VK_LWIN`.
- The four hoisted tests keep passing unchanged.

**Verification:** `cargo test` green on Linux, and
`XWIN_ACCEPT_LICENSE=1 cargo xwin check --target x86_64-pc-windows-msvc`
clean. The xwin gate is the one that proves the port; `cargo test` alone
cannot even compile `ll_hook.rs`.

⚠️ Patching only the four numbered lines is **not** the fix. Passing
`CaptureMode::Inject` at the two `RecordStart` sites makes it compile and
restores dictation, and leaves note capture with no Windows hotkey at all.

---

## Task 2: Remote-server resilience — split connect timeout, sweep failed notes

Two changes, both fully testable on Linux.

**2a. Split connect timeout from total timeout.** `src/llm/client.rs:20-23`
builds `reqwest::Client::new()` with no options, and `src/llm/mod.rs` has one
knob (`request_timeout_ms`, default 15 000). Over a tailnet these are two
different numbers: a short connect timeout so "desktop asleep" fails in
seconds instead of hanging 15 s per text run, and the existing generous total
timeout because waking Gemma costs 1.68 s and a cold server 4 s.

- Add `connect_timeout_ms: u64` to `LlmConfig` with
  `#[serde(default = "default_connect_timeout_ms")]` returning **5_000**.
  Measured RTT to the laptop was 13-289 ms (mdev 109, WiFi power saving), so
  2 s is too tight.
- Apply it with `.connect_timeout(Duration::from_millis(…))` when building the
  shared client in `http_client()`.
- ⚠️ `http_client()` is a `OnceLock` static, so whatever value is used at the
  first call is baked in for the process lifetime. **Ruling, already made: do
  not re-plumb the client through config.** Read the config inside
  `http_client()`'s initialiser (`Config::load()`, falling back to the default
  on error) and say plainly in the Local AI settings card that changing the
  connect timeout takes effect on restart.
- Surface the field in the Local AI settings card next to the existing
  request-timeout control, following that card's existing component patterns.

**2b. Sweep the backlog on the next success.** A failed pass sets `Failed` and
waits for a click (`src/notes/pipeline.rs:265-271`, `:366-371`); there is no
automatic retry anywhere today. The trigger must **not** be a timer — see the
never-poll constraint. Use the evidence you already have: a request that
succeeded proves the server is reachable.

- On a successful pass, re-send a `PipelineRequest` for every note whose
  `clean_state == StageState::Failed` or `extract_state == StageState::Failed`.
- `use_pipeline` already dedupes via `in_flight`
  (`src/notes/pipeline.rs:80-105`), so a sweep cannot storm the server.
- **Guard so a swept request does not itself trigger a sweep**, or a corpus of
  permanently failing notes re-sweeps forever. Add a field to
  `PipelineRequest` (for example `swept: bool`, defaulting false via a
  constructor) and skip the sweep when it is set.
- Ask each swept note for the stages that actually failed, not blindly
  `Stages::Both`: `Failed` clean + `Done` extract asks `CleanOnly`, and so on.
  A note with both failed asks `Both`.

**Tests to add** (pure, no server): a function that maps a `NoteStore` to the
list of `PipelineRequest`s a sweep should emit, covering — nothing failed
yields nothing; clean-failed yields `CleanOnly`; extract-failed yields
`ExtractOnly`; both failed yields `Both`; and every emitted request carries the
swept flag. Plus a test that a swept request's completion emits no further
sweep.

**Docs:** add the sweep to the failure-handling table in
`agent_docs/local_inference.md` and state why it does not violate the
never-poll rule. Add `connect_timeout_ms` and the tailnet URL
(`https://callisto.taila63f23.ts.net`) to `agent_docs/config_schema.md`, whose
text currently calls `base_url` "the ONLY connection setting".

**Verification:** `cargo test` green.

---

## Task 3: Windows runtime defects that compile clean but break the app

Three independent fixes. All are verified here by `cargo xwin check`; their
runtime behaviour needs the laptop.

**3a. `build.rs` guards on the host, not the target.** `build.rs:3` is
`#[cfg(target_os = "windows")]`, but inside a build script that tests the
**host**. Cross-compiled from Linux the whole block vanishes: no icon, no
`asInvoker` requestedExecutionLevel, no PerMonitorV2 manifest, and
`winresource` never runs and never complains.

Fix: guard on the target instead —
`if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") { … }` —
with the `winresource` use behind a `#[cfg]`-free runtime branch. `winresource`
is already a `[target.'cfg(windows)'.build-dependencies]`-style dependency;
check `Cargo.toml` and, if it is host-gated, make it an unconditional
build-dependency so a Linux host can link it for a Windows target. Keep the
manifest XML byte-identical.

Add a comment saying why the guard is a runtime `env::var` and not a `cfg!`,
because the `cfg!` version reads as obviously correct and is not.

**3b. `open_external` puts an unescaped URL through `cmd`.**
`src/ui/mod.rs:46` runs `cmd /C start "" <target>`. Rust only quotes arguments
containing spaces or quotes, so a URL containing `&` is split by the shell: the
browser gets a truncated URL and `cmd` tries to run the remainder as a
command. This is on the path for every sticky-note link chip and every `.ics`
export, and the URL comes from note content, so it is a command-injection
surface.

Fix: do not go through the shell parser. Prefer `ShellExecuteW` from the
`windows` crate (`Win32_UI_Shell` feature — add it to the existing `windows`
feature list in `Cargo.toml` if absent), with a null verb and `SW_SHOWNORMAL`,
passing the target as a wide string. `explorer.exe <target>` is the acceptable
fallback if `ShellExecuteW` proves awkward; it also skips the shell parser.
Keep the "spawn and detach, log a failure and do nothing else" behaviour and
the existing doc comment's reasoning.

Add a unit test for whatever string handling you introduce (for example
null-terminated wide conversion), runnable on Linux if the helper is
`cfg`-free, otherwise `#[cfg(target_os = "windows")]`.

**3c. `work_area()` reserves space for a panel that isn't there and none for
the taskbar.** `src/ui/work_area.rs:33` hardcodes `PANEL_INSET: i32 = 40` on
the top edge for the GNOME panel, and the union uses `MonitorHandle::size()`,
the full resolution — tao exposes no `work_area()`. On Windows notes get
placed under the taskbar while 40 px is wasted at the top.

Fix: behind `#[cfg(target_os = "windows")]`, get each monitor's usable
rectangle from `GetMonitorInfoW` → `MONITORINFO::rcWork` (needs
`Win32_Graphics_Gdi`), and use those rectangles in place of the
`size()`-derived ones, with `PANEL_INSET` applied only on non-Windows. The
existing pure `union_work_area` function and its tests stay as they are — it
takes already-logical `Rect`s, and only its caller changes. Keep the fallback
path.

Note in a comment that mixed-DPI multi-monitor placement remains wrong on
Windows (there is no global logical coordinate space and tao converts with a
single scale factor); uniform scale round-trips correctly, so one display or
two matched ones are fine. This is documented, not fixed.

**Verification:** `cargo test` green, `cargo xwin check` clean.

---

## Task 4: Serialize sticky-window opens, and the two accepted Windows limits

**4a. Serialize the opens.** `setup_sticky_windows`
(`src/ui/sticky_windows.rs:340-368`) spawns an `open_note_window` per restored
note, and dioxus drains **all** pending webviews in a single event-loop
iteration. On Windows each one calls
`CreateCoreWebView2EnvironmentWithOptions` then `wait_with_pump` — a nested
Win32 message pump on the main thread, once per note. This is the shape behind
dioxus#2483 ("Opening Multiple Windows on Desktop-Windows Fails 9 out of 10
Times", `WebView2Error(HRESULT(0x8007139F))`). On Linux the equivalent does
none of this.

**Ruling, already made: serialize unconditionally, not behind a `cfg`.** The
plan said "fix if it fires", but this session cannot see it fire, and the
existing slot-reservation design already picks positions synchronously up
front, so ordering cannot change placement. Serial restore on Linux costs
milliseconds.

Implement it by awaiting each `open_note_window` before dispatching the next —
one `spawn`ed task that loops over the pending notes in order, rather than one
`spawn` per note. Keep the synchronous slot reservation exactly where it is:
positions must still be chosen up front, before any await, so two notes can
never claim the same slot.

Add a comment naming dioxus#2483 and the HRESULT, so the next person does not
"optimise" the loop back into a fan-out.

**4b. Two limits to document, not fix.** Add these to
`agent_docs/text_injection.md` and `agent_docs/dioxus_architecture.md`
respectively (see Task 6 for the full docs sweep — put the prose there, this
task just makes sure the facts are not lost):

- *UIPI:* an unelevated `WH_KEYBOARD_LL` hook receives no input destined for a
  higher-integrity window, and `SendInput` into one is blocked. Beamer
  requests `asInvoker`, so dictation is silently dead in any elevated window.
  No fix exists short of a signed `uiAccess="true"` binary in Program Files.
- *Toasts:* `src/orchestrator/notify.rs:38` uses `Toast::POWERSHELL_APP_ID`, so
  notifications are branded "PowerShell" until an installer creates a Start
  Menu shortcut carrying a real AppUserModelID. `winrt-notification` has had no
  release since 2022-01-11; `tauri-winrt-notification` is the maintained fork.
  Do not swap the dependency in this task.

**4c. A product decision, already made.** Note windows omit
`with_skip_taskbar(true)` (unlike the splash and pill), so six open notes means
six taskbar buttons. **Ruling: keep them in the taskbar — no code change.**
Beamer's notes are deliberately not always-on-top, so skip-taskbar notes would
have no way back once buried, and Microsoft's own Sticky Notes appears in the
taskbar too. Record the decision in `agent_docs/sticky_notes.md` so it is not
relitigated.

**Verification:** `cargo test` green, `cargo xwin check` clean, and on Linux
the app still restores multiple notes (the change is cross-platform).

---

## Task 5: Callisto transport — `tailscale serve` in front of a loopback llama-server

Shell work on this machine, not code. llama-server has no authentication of
any kind, so it stays bound to `127.0.0.1:8080` exactly as it is and tailscaled
proxies it.

```bash
sudo loginctl enable-linger berkley     # user unit must survive a logout
systemctl --user daemon-reload          # unit changed in 40160f4
systemctl --user start llama-beamer
tailscale serve --bg 8080               # -> https://callisto.taila63f23.ts.net
```

Prerequisite: HTTPS certificates enabled for the tailnet (admin console →
DNS). Free on every plan; MagicDNS is already on. If `tailscale serve` refuses
because certs are off, that is an admin-console step no CLI can do — record it
as the one manual step and continue; no code work depends on it.

Then update the laptop's `%APPDATA%\Beamer\config.toml`:

```toml
[llm]
base_url = "https://callisto.taila63f23.ts.net"
```

`reqwest` is built with `native-tls`, so an `https://` base_url validates
against the OS trust store with zero code changes.

Rejected: `--host 0.0.0.0` puts an unauthenticated LLM API on every network
callisto joins. `--host 100.105.14.62` couples the unit to an address that
only exists once tailscaled is up, so it races the interface at boot. **Funnel
is never an option here** — it is the same command pointed at the public
internet.

**Verification:** `tailscale serve status` shows the proxy, and
`curl -s https://callisto.taila63f23.ts.net/v1/models | head` from callisto
returns a 200 listing both model ids.

---

## Task 6: Documentation corrections (Phase A)

Each of these is wrong today and costs time if believed. One dispatch, all
files.

- **`CLAUDE.md`** — the Task 4 entry calls the Windows hotkey work "a **build
  fix**, not parity work". That is stale and actively misleading: done as a
  build fix, note capture has no Windows hotkey. Replace with the accurate
  version (`todo.md` has it). Also: the "PICK UP HERE" block still names
  `feat/sticky-notes` as the branch carrying Phase 1; that work is on master
  and the branch is gone.
- **`todo.md`** — remove the claim that
  `cargo check --target x86_64-pc-windows-msvc` works here. It does not:
  `ring` enters the tree via `self_update` 0.42 (which force-enables reqwest's
  `rustls-tls`) and its build script wants MSVC `lib.exe`. Replace with the
  `cargo xwin` gate. Mark the two hotkey items done. Note that a **native**
  build on the laptop has `lib.exe` and is unaffected, so cross-compilation is
  the only thing that was ever blocked.
- **`agent_docs/dioxus_architecture.md`** — add the Windows facts that are not
  discoverable from the code: the nested Win32 message pump per webview and
  why window opens are serialized; the per-note WebView2 process cost (each
  control creates its own browser/renderer/GPU process set and wry creates an
  environment per webview with no caching, where WebKitGTK's shared-secondary
  -process model gives roughly one renderer for all notes, so Windows scales
  materially worse — measure `msedgewebview2.exe` RSS with N notes open); that
  dioxus disables browser accelerator keys unconditionally on Windows, killing
  Ctrl+F/F5/Ctrl+P in the webview; and the drag-and-drop differences (wry
  disables HTML5 DnD on Windows and dioxus synthesizes the events, so file
  drops work and `e.files()` returns real paths, but the synthetic
  `dataTransfer` carries no `text/uri-list` — dragging a URL from a browser
  onto a note silently does nothing — and `dragenter` is never synthesized, so
  the drop-target highlight at `src/ui/sticky.rs:194` is dead; the paperclip
  fallback is unaffected).
- **`agent_docs/text_injection.md`** — the UIPI limit from Task 4b: dictation
  is silently dead against elevated windows, permanently, at `asInvoker`.
  Also that `injection.paste_shortcut` is silently ignored on Windows
  (`src/injection/clipboard.rs:282` hardcodes Ctrl+V).
- **`agent_docs/config_schema.md`** — `connect_timeout_ms` and the tailnet URL
  (also covered by Task 2; make sure it is there and correct).
- **`agent_docs/local_inference.md`** — the sweep in the failure-handling
  table (also covered by Task 2) and the tailnet deployment from Task 5.
- **`agent_docs/sticky_notes.md`** — the taskbar ruling from Task 4c, and the
  Windows work-area behaviour from Task 3c.
- **`README.md`** — the paragraph saying the Windows target does not compile.

Also add a short **"Worth watching on Windows"** section to
`agent_docs/dioxus_architecture.md` for the unverified items, clearly marked
unverified: mixed-DPI multi-monitor placement; WebView2 user data folder is in
Roaming (`src/ui/mod.rs:56` uses `dirs::data_dir()`; Microsoft recommends
Local, it is a cache and roaming profiles will copy it); undecorated
transparent windows have no drop shadow (tao's `with_undecorated_shadow` is
never called); tray icon re-registration after an explorer restart is
unconfirmed (test with `taskkill /f /im explorer.exe`); and the main window's
fonts may not resolve, because `styles.css` uses relative `url("fonts/…")`
which `asset!()` does not track and it only works on Linux because the
resolver falls back to treating the URI as an absolute path (secondary windows
are immune, they use data URIs).

And a **"Cleared, so nobody re-investigates"** section: note images work on
Windows unchanged (`src/ui/sticky_blocks.rs` serves them via a root-relative
`/note-media/<id>` URL through `use_asset_handler`, and wry's URI work-around
makes the path identical on both platforms); `dm_mono_face_css()` emits data
URIs and is fine; the Windows recording pill path is structurally intact,
though `PILL_JS` animates from a CSS keyframe and so does not track mic level
the way the GNOME extension's pill does (a fidelity gap, not a break); there is
no localStorage/IndexedDB anywhere, so the whole class of origin-partitioning
problems is a non-issue; WebView2 is preinstalled on Windows 11; and
dioxus#3604 (mixed-DPI persisted positions corrupting until the app won't
launch) does not apply, because Beamer persists no positions by design.

**Verification:** the files say what this task says they should. No code
changes, so `cargo test` is unaffected but should still be green.

---

## Task 7: Split machine-local state out of the note, and give ids a machine component

This is the first Phase B task and the single highest-value change. It is
worth doing even if sync were abandoned.

**Why it matters.** `pos`, `size` and `open` describe a window on one screen,
not the note. `open` is the trap: `set_open` calls `touch()`, which rewrites
`modified` (`src/notes/mod.rs:195-203`). Under any last-write-wins scheme,
*closing a sticky on the laptop* makes that note newer than a real text edit on
the desktop and wins the merge. `set_size` is dirty-but-not-touched, so
resizing still generates sync churn with no content change.

**7a. Move `pos`, `size` and `open` out of `Note`** into a new machine-local
store persisted at `<config_dir>/machine.json`, keyed by note id. `set_open`
stops calling `touch()`. `NoteStore`'s public API should absorb the change:
the goal is that call sites see the same method names. Call sites to update:
`src/ui/sticky.rs`, `src/ui/sticky_blocks.rs`, `src/ui/sticky_windows.rs`,
`src/ui/notes_page.rs`, `src/notes/edit.rs`, `src/notes/lifecycle.rs`.

Garbage-collect entries for notes that no longer exist, on load.

**7b. Give note ids a machine component.** `next_id()`
(`src/notes/mod.rs:44-56`) is `{epoch_millis:x}-{counter:04x}` where the
counter is a per-process `AtomicU32` starting at 0, so the first note of every
session is `…-0000`. Two machines creating their first note in the same
millisecond produce **the same id**, and `Task.note_id` is a foreign key into
that namespace, so a collision silently reparents tasks onto the wrong note.

Keep the readable shape and append a short per-install random suffix persisted
in `machine.json` — for example `{millis:x}-{counter:04x}-{machine:04x}`. No
new dependency: seed the suffix from the system clock plus process id, or from
`std::collections::hash_map::RandomState`. The comment in `next_id()` about
avoiding a uuid dep stays true. Existing ids keep working — they are opaque
strings, and nothing parses them.

**7c. Migration.** An existing `notes.json` carries `pos`, `size` and `open` on
each note. On first load, lift those into `machine.json` and stop writing them
to the note. `Note` has no `deny_unknown_fields`, so the stale keys are ignored
rather than fatal; keep it that way.

**Tests:** id uniqueness across two different machine suffixes at the same
millisecond and counter; `set_open` does not change `modified`; migration lifts
`pos`/`size`/`open` off a legacy `notes.json` losslessly; machine-store GC
drops entries for absent notes.

**Docs:** `agent_docs/sticky_notes.md` — `Note::pos`/`size`/`open` move out of
the synced schema, and the "position persistence was dropped by decision"
section says where machine-local state lives now.

**Verification:** `cargo test` green; `cargo run --bin task_eval -- --limit 20`
still runs.

---

## Task 8: Own the attachment bytes

**This reverses a documented decision, deliberately.**
`src/notes/model.rs:85-93` and `src/notes/edit.rs:18-20` say Beamer never
copies an attachment's bytes, so "a note is never a second copy of your
library". That makes sync impossible: `Attachment::Image { path: PathBuf }`
stores `/home/berkley/Pictures/cat.png` verbatim from the drop event, so there
is nothing to sync and the path is meaningless on the other OS. Every image
becomes a "Locate…" card, and locating it on the laptop rewrites the path to a
Windows one that then breaks on the desktop. A ping-pong.

Copy on attach into `<config_dir>/sync/attachments/<sha256>.<ext>`; store the
hash and the original filename instead of a path.

**Half of the old decision survives and must stay true: deleting a note
deletes Beamer's copy and still never touches your original.** Rewrite both
doc comments to say the new rule. Do not quietly contradict them.

Content addressing means two notes referencing the same bytes share one file,
so deletion must be refcounted: only remove `<sha256>.<ext>` when no remaining
attachment references that hash.

Hashing: `sha2` is the obvious crate; check `Cargo.lock` first, because a
transitive copy may already be present and a direct dependency on the same
version costs nothing.

**Migration.** Existing notes carry paths. On first load, where the file still
exists, copy it in and rewrite the attachment. Where it does not, keep the
existing missing-file "Locate…" card rather than dropping the attachment — a
broken reference is recoverable, a deleted one is not.

**Tests:** the same bytes attached twice yield one file and one hash;
deleting one of two notes referencing the same hash keeps the file; deleting
the last one removes it; deleting a note never touches the original source
path; migration of a legacy path-based attachment whose file exists; migration
of one whose file is missing keeps the attachment.

**Verification:** `cargo test` green.

---

## Task 9: Automerge document, merge before flush, JSON mirror

**Why automerge.** Version 0.11.0 shipped 2026-08-12; the Rust crate is the
primary implementation, maintained full-time at Ink & Switch, and Automerge 3
cut memory use roughly 10x. What it buys that last-write-wins does not:

- `ObjType::Text` merges character-level edits across machines, so two machines
  editing the same note offline merge by character instead of one overwriting
  the other.
- `merge()` is deterministic regardless of how the bytes arrived. That is the
  load-bearing property, and it dissolves the transport problem in Task 10.
- History, without git's orchestration or conflict markers a JSON parser
  cannot read.

⚠️ **Text merging only works if the writes are splices.** `src/ui/sticky.rs`
writes `body` on every keystroke; a whole-string `put` per keystroke makes the
field an LWW register and you carry automerge's weight for none of its
merging. Use `update_text` (diff-based splicing) so a keystroke becomes a
splice.

⚠️ **The cleanup compare-and-swap stays.** Character-merging a model's
wholesale rewrite against a user's mid-flight edit yields interleaved text that
is neither, which is strictly worse than today. Automerge's text merge is for
concurrent *user* edits on two machines. The cleanup-vs-typing race keeps its
current semantics: the user's edit wins and the pass reports `Superseded`. Do
not "simplify" `src/notes/lifecycle.rs::apply_cleanup` away.

**9a. Back the note and task corpus with an automerge document** persisted at
`<config_dir>/sync/notes.automerge`. `NoteStore` and `TaskStore` keep their
public APIs.

**9b. Merge before flush.** Today the stores load once into signals
(`src/ui/app.rs:62-65`) and a 500 ms tick rewrites the entire JSON
(`src/ui/app_setup.rs:273-286`, `src/notes/mod.rs:98-114`). An externally
synced file is clobbered without a word. Worse, a merge that produces invalid
JSON is silently renamed to `notes.json.corrupt` and replaced with an **empty
store** (`src/notes/mod.rs:83-88`), and no error reaches the UI.

Before each flush, if the file's mtime moved since our last write, load the
incoming document and `merge()` rather than overwrite. A `notify`-based
watcher for live updates is a later nicety; the mtime check is the part that
prevents data loss. Also: a load failure must surface to the UI status log
rather than silently substituting an empty store.

**9c. Keep a JSON mirror.** `src/bin/task_eval.rs` reads `notes.json` and
`tasks.json` with its own envelope structs and cannot use crate-rooted paths.
Write `notes.json`/`tasks.json` as derived, never-read exports so `task_eval`
keeps working and the store stays eyeball-debuggable.

**Tests:** two divergent documents, both edited offline, `merge()` produces
both edits; the same note edited on both merges character-level with nothing
lost; a keystroke produces a splice rather than a whole-string put; the mtime
check merges instead of clobbering; a corrupt document surfaces an error
instead of an empty store; round-tripping the existing 14-note `notes.json`
into the new store and back out to the JSON mirror is lossless.

**Verification:** `cargo test` green; `cargo run --bin task_eval -- --limit 20`
still runs against the mirror.

---

## Task 10: Sync directory layout and conflict-copy merge

Because `merge()` is order-independent, the sync protocol is optional.
Syncthing over the tailnet carries the files. It is installed on neither
machine yet (`apt install syncthing` here, the Windows installer there, then
pair the two devices by their tailnet addresses).

```
%APPDATA%\Beamer\sync\   (or ~/.config/Beamer/sync/)   <- the Syncthing share
    notes.automerge          shared note + task corpus
    attachments/<hash>.<ext> content-addressed bytes
%APPDATA%\Beamer\            <- NOT shared
    config.toml              machine-specific, see below
    machine.json             pos / size / open, per machine
    history.json             dictation history, machine-local
```

Moving the synced data into a `sync/` subdirectory is what makes a
**folder-scoped** share safe on Windows, where `config_dir()` and
`webview_data_dir()` are the same folder (`%APPDATA%\Beamer`,
`dirs-6/src/win.rs:8,10`) and a folder-scoped sync would otherwise drag the
WebView2 browser profile along.

**Conflict copies.** If Syncthing leaves
`notes.sync-conflict-*.automerge` beside the document, Beamer loads each one,
`merge()`s it, saves, and deletes it. Conflicts stop being fatal and become
routine. Match the glob strictly enough that an unrelated file is never
deleted.

**No sign-in, no account.** The tailnet already authenticates these devices; an
account would duplicate auth that exists and put the notes on someone else's
server. Task 11 adds a live `automerge::sync` path on top of this one; file
sync stays as the fallback and as the on-disk format either way. Note that
`automerge_repo`'s wire format is **not** compatible with the JS
implementation, so do not reach for the JS sync server.

**API keys** are keyring-only and cannot sync. Enter them once per machine;
say so in the docs.

**Do not sync `config.toml`.** Syncing it is actively harmful:
`injection.backends` is OS-branched with no validation that a stored name is
valid for the running OS, so a Linux chain on Windows fails every backend;
`appearance.auto_start` is applied at every launch (`src/main.rs:39-43`), so
syncing `true` silently enrols the laptop; `llm.base_url` is per-machine by
definition; and `notes.all_workspaces` and `injection.paste_shortcut` are
Mutter/Linux-only. A later `preferences.toml` (synced) vs `machine.toml` (not)
split is the clean answer if shared settings are wanted, but it is not on the
critical path and it needs the non-atomic `fs::write` at
`src/config/mod.rs:234` fixed first. Record that as a `todo.md` item.

**Tests:** a conflict copy is merged and removed; an unrelated file matching a
loose glob is not removed; the sync directory is created if absent; paths
resolve correctly on both platforms (test the path-building function, which
should be `cfg`-free).

**Verification:** `cargo test` green.

---

## Task 11: Live sync with `automerge::sync`, falling back to file sync

Task 10 leaves a working file-sync path. This task tries the better one and
keeps the fallback if it does not pan out. **Spike first, then decide** —
that is the task, not "ship live sync regardless".

`automerge::sync` (`automerge::sync::{State, SyncDoc, Message}`) runs over any
reliable, in-order, message-framed transport. `tokio-tungstenite` 0.26 with
`native-tls` is **already a dependency** (`Cargo.toml:30`), so a WebSocket
costs no new crate on either end.

**Shape.**

- A small server binary, `src/bin/sync_server.rs`, runs on callisto beside
  llama-server. It owns the authoritative `notes.automerge`, accepts WebSocket
  connections, and runs one `sync::State` per peer. Reuse the file-sync
  document format from Task 9 verbatim — the server is another replica, not a
  new schema.
- Beamer connects out to `wss://callisto.taila63f23.ts.net/sync`, published by
  `tailscale serve --bg --set-path /sync 8081` (a **second** port; `/` is
  already llama-server from Task 5). Confirm `tailscale serve` proxies a
  WebSocket upgrade — if it does not, that is a fallback trigger, not a
  workaround to hunt.
- Binary frames carry `Message::encode()` / `Message::decode()` output
  directly. No JSON envelope.
- **No auth beyond the tailnet.** Same reasoning as llama-server: tailscaled
  is the authenticator, the listener binds loopback, and Funnel is never an
  option. The server must refuse to start if asked to bind anything but
  loopback.
- The client is offline-first: a failed or dropped connection is normal and
  silent, with reconnect backoff. Notes keep working with no server. **The
  local `notes.automerge` stays the source of truth on each machine**, so
  losing the server loses live propagation and nothing else.
- ⚠️ Dioxus `spawn`, never `tokio::spawn`, for anything holding a `Signal`.
  The socket task itself holds no signals: have it own a channel and let a
  Dioxus-side coroutine apply incoming changes.

**Fallback triggers — any one of these means stop and keep Task 10's file
sync.** Record which one fired.

- `tailscale serve` will not proxy the WebSocket upgrade.
- Live sync and the 500 ms flush cannot be reconciled without the document
  being written from two places at once.
- The spike does not converge in a working state within this task's fix-round
  budget.

If it works, keep file sync too. It costs almost nothing once the document
format exists, it covers the case where callisto is off, and it is the reason
this task is allowed to fail safely.

**Tests:** a `sync::State` round trip between two in-process documents
converges (no socket needed — this is the part worth testing and the only part
that is deterministic); a dropped connection mid-exchange leaves both documents
valid and re-converges on reconnect; the server refuses a non-loopback bind.

**Verification:** `cargo test` green. Two Beamer instances against a local
`sync_server` converge — runnable here with two config dirs on this one
machine, which is the honest version of the two-machine test. The real
two-machine run stays on the manual list.

---

## Task 12: Documentation corrections (Phase B)

- `agent_docs/sticky_notes.md` — machine-local state, finished from Task 7.
- `src/notes/model.rs:85-93` and `src/notes/edit.rs:18-20` — the "never
  copies" decision is reversed by Task 8, with the surviving half stated
  plainly.
- `CLAUDE.md` — the storage layout, the `sync/` directory, and what does not
  sync (config, keyring, history).
- A new `agent_docs/sync.md` covering the design: why automerge, why file sync
  rather than a server, the conflict-copy path, and the Syncthing setup on both
  machines.
- `todo.md` — the deferred `preferences.toml` split and the non-atomic
  `fs::write` at `src/config/mod.rs:234`.

**Verification:** the files say what this task says they should.

---

## Verification that this session cannot run

State these plainly rather than claiming them. They need hardware this session
does not have, and an implementer must never report them as verified.

Needs the laptop (bearcave):

1. Build on Windows, set `base_url`, open Settings → Local AI: both models and
   their states list.
2. Dictate a note with callisto reachable: it cleans, chips appear.
3. `systemctl --user stop llama-beamer` on callisto (**never** `pkill -f`,
   which matches the shell running it), dictate three notes: all captured, all
   showing the red footer label.
4. Start the server, dictate a fourth: it cleans, **and the three stale notes
   clean themselves with no click.** That is the sweep.
5. The dictation hotkey still injects into a focused field via the
   UIA/SendInput chain, and then the **note** hotkey works — the one that would
   silently not exist if Task 1 were done as a four-line patch.
6. Restore five or more notes at once: no `HRESULT(0x8007139F)`, no panic.
7. Notes are not placed under the taskbar, and no 40 px is wasted at the top.
8. Click a link chip whose URL contains `&`: the browser gets the whole URL and
   no stray `cmd` window appears.
9. Right-click `beamer.exe` → Properties shows the icon (and
   `mt.exe -inputresource:beamer.exe -out:extracted.xml` for the manifest).
   On a cross-compiled binary this is the check that would otherwise silently
   fail.
10. Dictate into an elevated window and confirm nothing happens. Expected, and
    the reason it needs documenting rather than debugging later.

Needs both machines with Syncthing:

11. Create a note on each while both are online: both appear on both.
12. Create notes on both with Syncthing stopped, then start it. Both survive.
    Force a conflict copy and confirm Beamer merges and removes it rather than
    quarantining to `.corrupt`.
13. Attach an image on the laptop: it renders on the desktop. Delete the note:
    Beamer's copy goes, the original file does not.
14. Open a note on the desktop: it does not open on the laptop and does not
    bump `modified`.
15. Cleanup-vs-typing still behaves as it does today: type while a pass is in
    flight and the pass reports `Superseded` with your text intact, **not** a
    character-level interleave.

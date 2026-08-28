# Dioxus Architecture

There is no Mica backdrop wiring — that was never built. Multi-window is real,
and grew: one main window (settings/home UI), an optional recording pill window
on Windows/macOS, a splash window during startup, **one sticky note window per
open note**, and (Linux only) an in-shell indicator driven over D-Bus instead of
a Dioxus window at all.

Sticky notes are the only place multiple long-lived `VirtualDom`s share state,
and the rules for doing that safely are not obvious — `use_context` does not
cross the boundary, `GlobalSignal` silently diverges per window, and event
handlers are per-window. **Read `agent_docs/sticky_notes.md` before touching
multi-window code.**

## Threading Model

**Critical, unchanged:** `dioxus::launch()` (called from `launch_app()` in
`src/ui/mod.rs`) blocks the main thread and owns the single tokio runtime
for the whole process.

```
Main thread (Dioxus/tao owns)
├── tao event loop
├── tray-icon events
└── Dioxus rendering / webview

Tokio runtime (Dioxus-managed — the only one)
├── orchestrator coroutine (hotkey → audio → transcription → injection)
├── audio capture channel plumbing
├── transcription HTTP/WebSocket calls
└── injection (tokio::task::spawn_blocking — see below)
```

Rules:
1. Never create a second `#[tokio::main]` or `Runtime::new()`.
2. Use `tokio::spawn()` / `spawn()` (Dioxus re-export) for async work inside
   hooks.
3. Blocking work — anything touching UIA/Win32 COM, or the Linux zbus D-Bus
   calls in `src/injection/focus.rs` / `gnome.rs` — goes through
   `tokio::task::spawn_blocking`. `injection::inject_text` (called from
   `orchestrator.rs`) wraps the entire fallback chain in one
   `spawn_blocking(move || inject_text_blocking(...))` call
   (`src/injection/mod.rs`).

## App Structure (`src/ui/`)

- `mod.rs` — `webview_data_dir()`, and `launch_app()`: builds the window
  icon, configures the main `WindowBuilder` (starts hidden, transparent, no
  native decorations — the splash window owns the launch moment), and calls
  `LaunchBuilder::new().with_cfg(...).launch(app::App)`.
- `app.rs` — the `App` component itself: owns all top-level `Signal`s
  (`current_page`, `rec_state`, `history`, `config`, `status_log`,
  `update_status`), wires the low-level keyboard hook to the orchestrator
  coroutine, and renders the sidebar/titlebar/page shell. `Page` is a plain
  enum (`Home`/`History`/`Vocab`/`Settings`) switched on in the render body.
- `app_setup.rs` — every startup/window-management concern extracted out of
  `App()` into standalone functions (`setup_tray_menu`,
  `setup_window_centering`, `setup_splash`, `setup_recording_pill`,
  `setup_update_check`, `setup_menu_handlers`, `setup_tray_click_handler`),
  each called once from `App()`'s render body. See "Hook Order" below for
  why this split is safe.
- `linux_integration.rs` (`#![cfg(target_os = "linux")]`) — swaps the tray
  icon between idle/recording glyphs and drives the GNOME Shell pill (see
  `shell_indicator.rs`) instead of a floating window: GNOME doesn't let
  standalone apps host in-tray widgets, and floating always-on-top windows
  have compositor/transparency quirks under Wayland. Also runs the 66ms
  level-pump loop described in `audio_pipeline.md`.
- `pill.rs` — the Windows/macOS recording pill's Dioxus component + CSS/JS
  (`#[cfg(not(target_os = "linux"))]`), a dark-glass capsule with a
  waveform/timer, driven by a `beamerSetState('recording'|'processing'|'idle')`
  JS call from `app_setup.rs`'s `setup_recording_pill` effect — not by
  Dioxus state directly, since it lives in a separate `VirtualDom`/window.

## Hook Order Constraint

`app_setup.rs`'s functions are Dioxus hooks (`use_hook`/`use_signal`/
`use_effect`) extracted **verbatim** out of `App()` into standalone
functions, each still called exactly once, in the same order, on every
render of `App()`. Dioxus (like React) requires hooks to run in a fixed
order across renders — reordering, conditionally skipping, or looping these
calls in `App()`'s body will corrupt hook state. If you need to add a new
piece of startup wiring, add a new `setup_*` function and call it from
`App()` in a fixed position; don't wrap existing calls in conditionals.

## Data Flow: Props, Not Context

There's no `AppState`/context-provider pattern. `App()` owns the `Signal`s
and passes them down as **component props** — e.g. `SettingsPageProps`
(`src/ui/settings/mod.rs`) carries `config: Signal<Config>`,
`last_injection: Signal<String>`, `status_log: Signal<StatusLog>`,
`update_status: Signal<UpdateStatus>` explicitly; `HomePageProps` carries
`rec_state`/`history`/`config`. `Signal<T>` is `Copy`, so passing it as a
prop doesn't clone the underlying data — components read/write the same
signal the parent holds. Settings-card components (`src/ui/settings/*.rs`)
follow the same pattern one level deeper.

Cross-task communication (hotkey → orchestrator, live mic level →
shell-indicator pump) uses plain tokio channels (`mpsc`, `watch`), not
Dioxus signals — see `audio_pipeline.md` for the level-`watch::channel` and
`orchestrator.rs` for the hotkey `mpsc`.

## Memoized Derived Data

Expensive per-render recomputation is wrapped in `use_memo`, not
recalculated inline:
- `history_page.rs`: `use_memo(move || history.read().grouped_by_day())` —
  groups history entries by calendar day.
- `vocab_page.rs`: `filtered = use_memo(...)` over the vocab list + filter
  text.

## Tray Icon Integration

`tray-icon` (via `dioxus::desktop::trayicon`) is set up once in
`app_setup::setup_tray_menu` (`use_hook`), before any rendering-dependent
state exists. Tray menu clicks are wired through
`use_muda_event_handler` — **not** `use_tray_menu_event_handler` — because
dioxus-desktop 0.7.3 has a bug where `set_menubar_receiver()` claims the
muda `OnceCell` before `set_tray_icon_receiver()`, so tray menu clicks
arrive as `MudaMenuEvent` rather than `TrayMenuEvent` (see the comment in
`app_setup.rs` above `setup_menu_handlers`). Left-click-to-toggle-window
uses `use_tray_icon_event_handler` separately (`setup_tray_click_handler`).

## Splash Window

`app_setup::setup_splash` opens a small centered `VirtualDom`/window on
first render, runs `warmup::warm_all` (keyring, audio device, mpris,
network — paying one-time costs up front so the first recording doesn't
stall), waits for a 1500ms CSS fill animation to finish, closes the splash
window, then reveals the main window (except on Windows, which stays hidden
until a tray click, matching the original tray-app convention).

The network step **matches on the backend name exhaustively**. Realtime
backends (`elevenlabs`, `voxtral`) open and drop a real session, which is
what they'd do on the first recording anyway. Batch backends
(`elevenlabs_batch`, `voxtral_batch`) get
`transcription::preconnect_batch_host` instead — an unauthenticated GET that
warms DNS/TLS/the shared client's connection pool with no billable side
effect. Do not reintroduce a `_ =>` fallback here: it previously routed the
batch backends into the ElevenLabs *realtime* constructor, so every launch
opened a metered realtime STT session for users who had never selected one.

## Windows: things Linux never has to think about

Everything below is Windows-only and mostly invisible from either platform's
code in isolation. `cargo xwin check` proves these paths compile; none of
their runtime behaviour has been exercised on a real Windows machine, and
that is called out explicitly wherever it matters.

### Every webview open is a nested Win32 message pump

On Windows, opening a webview means wry calls
`CreateCoreWebView2EnvironmentWithOptions` and then blocks the calling thread
in `webview2_com::wait_with_pump`, a nested Win32 message pump, once per
window. `setup_sticky_windows` used to spawn one `open_note_window` task per
restored note, and dioxus drains every pending webview in a single
event-loop iteration, so restoring several notes at once drove several of
these nested pumps back to back on the main thread. That is the exact shape
behind dioxus#2483 ("Opening Multiple Windows on Desktop-Windows Fails 9 out
of 10 Times", `WebView2Error(HRESULT(0x8007139F))`). `src/ui/sticky_windows.rs`
now reserves every slot and position synchronously up front (so ordering
still can't change where a note lands), then opens the windows one at a time
from a single spawned task instead of fanning them out. Linux's equivalent
does none of this (WebKitGTK has no comparable nested-pump step), so resist
"simplifying" the restore loop back to a fan-out; it costs Linux nothing and
breaks Windows in a way that is hard to reproduce outside a multi-note
restore.

Whether the serialized restore actually clears the `0x8007139F` failure at
runtime is unverified; it removes the mechanism the dioxus issue describes,
but nobody has restored several notes on a Windows machine yet to confirm it.

### Per-note WebView2 process cost

Each WebView2 control creates its own set of browser/renderer/GPU processes,
and wry creates a fresh WebView2 *environment* per webview with no caching
between them. A sticky note is one webview each, so N open notes on Windows
means roughly N times the WebView2 process overhead. WebKitGTK's model on
Linux is the opposite: a shared secondary-process pool gives roughly one
renderer across every note window, so the same N-notes scenario costs
Windows materially more than it costs Linux. Not measured: `msedgewebview2.exe`
RSS with several notes open would need to be read off the laptop; nothing
here quantifies how bad the gap actually is, only that the architecture
guarantees there is one.

### Browser accelerator keys are off, everywhere, unconditionally

dioxus-desktop calls `webview.with_browser_accelerator_keys(false)` behind a
bare `#[cfg(target_os = "windows")]` with no further gate
(`dioxus-desktop-0.7.10/src/webview.rs:403-408`), so every Windows webview,
main window and every sticky note alike, loses the browser's built-in
Ctrl+F, F5, and Ctrl+P handling. This is dioxus's own default, not something
Beamer's code opts into or could opt out of without patching the dependency.
It should not surprise anyone that these shortcuts do nothing in a note or
in Settings on Windows; there is no bug to chase here.

### Drag-and-drop diverges between platforms

`.sticky-bar`'s drop handling (`src/ui/sticky.rs:191-206`) reads the same
way on both platforms, but wry backs it differently underneath. dioxus-desktop
0.7.10's `with_drag_drop_handler` (`dioxus-desktop-0.7.10/src/webview.rs:411-412`)
is installed unconditionally on every platform; only `disable_file_drop_handler`
(default `false`) would suppress it. On every platform but Windows that native
handler just merges real dropped files into the synthesized `DragData`, so
`e.files()` is non-empty exactly when a real file was dropped. Windows is the
one where the native handler causes a problem: per the comment at
`webview.rs:328-330`, "Windows webview blocks HTML-native events when the drop
handler is provided", so dioxus glue code mimics drag-drop events instead,
wiring `handleWindowsDragDrop` / `handleWindowsDragOver` / `handleWindowsDragLeave`
in `launch.rs:61-83` off the native `DragDropEvent`. `e.files()` still returns
real paths on Windows because that glue code attaches a real `File` to the
synthesized `drop` event, so the file-drop path works the same on both
platforms. What does not carry over is the URL case. `attachments_from_drop`
falls back to `dataTransfer.getData("text/uri-list")` when there are no files,
and Windows's synthetic `dataTransfer` never populates that field, so dragging
a URL from a browser onto a note silently does nothing on Windows even though
it attaches a link chip on Linux. Worse for visual feedback, `ondragenter`
(`sticky.rs:194`, the handler that flips `drop_target` and shows the
drop-target highlight) is never synthesized on Windows at all, so that
highlight is dead code there. The interpreter shim (`dioxus-interpreter-js-0.7.10/src/ts/native.ts:256-321`)
only ever dispatches `dragover`, `dragleave`, and `drop` from the Windows glue
path, with no `handleWindowsDragEnter` counterpart at all, so there is no code
path by which a synthesized `dragenter` could fire on Windows. None of this
touches the paperclip button, which opens a native file dialog and is
unaffected on every platform.

### Toast branding

`src/orchestrator/notify.rs:39` constructs notifications with
`winrt_notification::Toast::POWERSHELL_APP_ID`, so every Beamer notification
on Windows is branded "PowerShell" in the notification center until an
installer exists that creates a Start Menu shortcut carrying a real
AppUserModelID. `winrt-notification` has had no release since 2022-01-11;
`tauri-winrt-notification` is the maintained fork, but swapping it is not
scoped to any task currently in flight. This is a known, accepted rough
edge, not a bug to fix opportunistically.

### The `build.rs` host-vs-target trap

`build.rs` used to guard its icon/manifest/`winresource` block with
`#[cfg(target_os = "windows")]`. Inside a build script that attribute
evaluates for the **host** compiling the script, not the target the final
binary is built for, so cross-compiling from this Linux box silently
dropped the embedded icon, the `asInvoker` `requestedExecutionLevel`, and
the PerMonitorV2 manifest, and `winresource` never ran and never
complained. Nothing about that failure shows up in `cargo xwin check`
output; the block just quietly does not exist. The fix reads
`CARGO_CFG_TARGET_OS` from the environment at build-script runtime instead,
which reflects the actual target, and moves `winresource` off a
target-gated `build-dependencies` entry so a Linux host can still link it in
for a Windows target. The general trap, a build script's `#[cfg(...)]`
answering for the host and not the target, is worth remembering anywhere
else `build.rs` grows a platform branch.

### `open_external` no longer goes through a shell

Task 3b closed a command-injection surface in `ui::open_external` on
Windows (a `cmd /C start` argument split reachable from note content, a
link chip or an `.ics` export). `src/ui/mod.rs:72-90` documents the fix
in full; nothing here restates it. What the comment doesn't say: whether
`ShellExecuteW` actually opens links and files correctly at runtime is
unverified, same as everything else in this section that needs the laptop.

### Auto-repeat: why `ll_hook.rs` and `linux_hotkey.rs` guard differently

Both hotkey backends share the same platform-neutral matching layer
(`build_bindings`, `matching_binding`, `BindingConfig`, `BindingState`,
hoisted into `hotkey/mod.rs`), but they call `matching_binding` at different
points, and that difference is load-bearing, not stylistic.

`KBDLLHOOKSTRUCT`, what a Windows low-level keyboard hook receives, carries
no repeat or previous-key-state bit. Windows resends `WM_KEYDOWN` on every
auto-repeat tick of a held key, so `ll_hook.rs` has no way to tell an
auto-repeat apart from a fresh press except by tracking state itself.
`linux_hotkey.rs` never faces this: evdev tags a repeat with `value == 2`,
and `handle_key_event` returns immediately when the event is neither a press
nor a release (`value == 0` / `value == 1`), so a repeat never reaches
`matching_binding` at all.

That is why `ll_hook.rs` keeps a cross-binding, physical-trigger-keyed
`already_held` guard that runs **before** `matching_binding`
(`src/hotkey/ll_hook.rs:122-127`), where `linux_hotkey.rs` calls
`matching_binding` first and only checks per-binding held state **after**.
A binding-per-binding guard, matching Linux's shape, looks like the more
natural port. It does not hold up: traced through the two files, the
counterexample is holding Ctrl+Super (dictation) and tapping Alt mid-hold. A
`VK_LWIN` auto-repeat during that tap re-reads live modifier state via
`GetAsyncKeyState`, resyncs to `ctrl+alt`, and, with a per-binding
post-match guard, would match the sibling `Ctrl+Alt+Super` (note) binding
and fire `RecordStart(Note)` with no fresh keypress at all, stranding that
binding's `trigger_held`/`armed` state forever, because the release path's
`find` only clears the *first* binding it finds still marked held for that
physical key. Gating on the physical trigger key before matching is what
keeps "at most one binding held per physical key" true, and that invariant
is what makes the release path's first-match `find` safe. If a future
refactor "simplifies" the Windows guard to look like Linux's, it
reintroduces this bug: the two backends genuinely need different guard
placement, not the same guard ported carelessly.

## Worth watching on Windows

None of the following has been exercised on a Windows machine. They are
flagged so a future session investigating a Windows bug in one of these
areas does not have to start from zero, not because any of them is known to
be broken.

- **Mixed-DPI multi-monitor placement.** `ui::work_area::work_area` divides
  each monitor's physical rectangle by that monitor's own scale factor and
  unions the results as though they shared one logical coordinate space.
  That is only actually true when every monitor uses the same scale. A
  single display, or several matched displays, round-trips correctly; a
  genuinely mixed-DPI pair does not, and modelling Windows' real per-monitor
  virtual-desktop layout was judged a bigger change than the taskbar-aware
  fix it rides alongside.
- **WebView2's user data folder lives in Roaming, not Local.**
  `webview_data_dir()` (`src/ui/mod.rs:170`) uses `dirs::data_dir()`, which
  resolves to `%APPDATA%` (Roaming) on Windows. Microsoft's own guidance is
  to put a WebView2 user data folder under `%LOCALAPPDATA%`: it is a cache,
  and a roaming profile will copy it across machines for no benefit.
- **Undecorated transparent windows have no drop shadow.** Sticky notes are
  built `with_decorations(false)` for the square-cornered/rounded-corner
  trick documented in `agent_docs/sticky_notes.md`. tao exposes
  `with_undecorated_shadow` for exactly this case on Windows and it is never
  called, so notes likely render with a harder edge there than the
  `box-shadow` CSS alone can fake, unlike the compositor-drawn shadow this
  trick doesn't need on Linux.
- **Tray icon re-registration after Explorer restarts.** Windows sends
  `TaskbarCreated` when `explorer.exe` restarts, and tray icons that don't
  re-register in response vanish until the app itself restarts. Whether
  Beamer's tray setup handles this is unconfirmed; the test is
  `taskkill /f /im explorer.exe` (it restarts itself) followed by checking
  whether the tray icon survives.
- **The main window's fonts may not resolve at all.** `assets/styles.css`
  loads its `@font-face` sources with a relative `url("fonts/…")`, and
  Dioxus's `asset!()` machinery does not track plain CSS `url()` references,
  only assets it is explicitly told about. This currently works on Linux
  only because the resolver falls back to treating the unrecognised URI as
  an absolute filesystem path, which happens to land somewhere valid there.
  Nothing says that fallback behaves the same way on Windows. Secondary
  windows (stickies, pill, splash) are immune regardless. They get their
  fonts from `ui::fonts::embedded_font_css()`'s `data:` URIs, which cannot
  fail to resolve on any platform.

## Cleared, so nobody re-investigates

- **Note images work on Windows unchanged.** `src/ui/sticky_blocks.rs`
  serves attachment images through a root-relative `/note-media/<id>` URL
  via `use_asset_handler`, and wry's URI handling makes that path resolve
  identically on both platforms.
- **`ui::fonts::embedded_font_css()` is fine as-is.** It emits `data:` URIs
  with the font bytes inlined, which is exactly the kind of reference that
  cannot fail to resolve regardless of platform or windowing quirks.
- **The Windows recording pill path is structurally intact**, but not at
  full fidelity: `PILL_JS`'s waveform bars animate from a CSS `@keyframes`
  loop keyed only to `beamerSetState('recording'|'processing'|'idle')`, with
  no level parameter at all, unlike the GNOME extension's pill, which is fed
  real levels through `UpdateLevel(d)`. That is a fidelity gap the Windows
  pill has always had, not something this round of work broke.
- **No `localStorage` or `IndexedDB` use anywhere in the codebase.** The
  whole class of Chromium/WebKit origin-partitioning problems that trips up
  apps relying on per-webview storage does not apply here; there is nothing
  stored client-side to partition.
- **WebView2 is preinstalled on Windows 11**, so there is no first-run
  "please install the runtime" step to design around on the primary target
  OS.
- **dioxus#3604 (mixed-DPI persisted window positions corrupting until the
  app won't launch) does not apply to Beamer.** That bug is about restoring
  a *persisted* position across a DPI change; Beamer persists no window
  positions by design (see `agent_docs/sticky_notes.md`, "Scope change:
  notes are placed, not remembered"), so there is nothing for a stale DPI
  value to corrupt.

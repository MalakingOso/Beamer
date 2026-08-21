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

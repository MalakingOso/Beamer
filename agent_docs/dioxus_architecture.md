# Dioxus Architecture

## Threading Model

**Critical:** Dioxus 0.7 `launch()` is blocking and owns both the main thread and a tokio runtime.

```
Main Thread (Dioxus owns)
├── tao event loop (Windows message pump)
├── tray-icon events
├── global-hotkey events
└── Dioxus rendering

Tokio Runtime (Dioxus-managed)
├── Audio capture task
├── Transcription API calls
├── WebSocket connections
└── Text injection (spawn_blocking)
```

### Rules
1. **Never** create a second `#[tokio::main]` or `Runtime::new()`
2. Use `tokio::spawn()` for async background work from within Dioxus hooks
3. Use `tokio::task::spawn_blocking()` for COM/Win32 calls
4. The main thread runs the tao event loop — don't block it

## App Startup Sequence

```rust
fn main() {
    // 1. Initialize logging
    // 2. Load config
    // 3. Set up tray icon (tray-icon crate, runs on main thread via tao)
    // 4. Register global hotkeys (global-hotkey crate)
    // 5. Launch Dioxus (blocks main thread)
    dioxus::launch(App);
}
```

## Multi-Window Management

Dioxus 0.7 supports multiple windows via `WebviewWindowBuilder`:

### Settings Window (primary)
- Created by `dioxus::launch()`, starts hidden
- Toggle visibility from tray menu
- Custom title bar (no native decorations)

### Overlay Window
- Created via `WebviewWindowBuilder::new()` from a Dioxus hook
- Transparent, always-on-top, no decorations
- Positioned bottom-center of screen
- Shows/hides based on recording state

### Glow Window
- Fullscreen transparent window, always-on-top
- Click-through via `WS_EX_TRANSPARENT`
- CSS-rendered colored border
- Shows/hides based on recording state

## Inter-Component Communication

Use tokio channels for cross-component communication:

```rust
// Shared app state via Dioxus signals
#[derive(Clone)]
struct AppState {
    config: Signal<Config>,
    is_recording: Signal<bool>,
    last_transcript: Signal<String>,
    injection_debug: Signal<String>,
}
```

For background tasks, use `mpsc` channels:
- Hotkey events → audio pipeline control
- Audio chunks → transcription backend
- Transcript text → injection + overlay update

## Tray Icon Integration

`tray-icon` crate works with `tao` event loop natively. Set up before Dioxus launch:

```rust
let tray = TrayIconBuilder::new()
    .with_icon(icon)
    .with_menu(menu)
    .with_tooltip("Beamer")
    .build()?;
```

Handle menu events via `MenuEvent::receiver()` in a spawned task.

## Mica Backdrop

Post-window-creation, get the HWND and call:
```rust
DwmSetWindowAttribute(
    hwnd,
    DWMWA_SYSTEMBACKDROP_TYPE,
    &DWM_SYSTEMBACKDROP_TYPE::Mica as *const _ as *const _,
    size_of::<i32>() as u32,
);
```

This requires the webview background to be transparent.

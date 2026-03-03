# Beamer

Windows system-tray dictation app. Press a hotkey, speak, and your words appear in whatever text field has focus.

Beamer captures microphone audio, streams it to a cloud speech-to-text API over WebSocket, and injects the transcript into the active input using Windows UI Automation.

## Tutorials

### Getting started

This walkthrough takes you from a fresh clone to your first dictation in about five minutes.

**1. Prerequisites**

- Windows 10/11
- [Rust toolchain](https://rustup.rs/) (stable, MSVC target)
- An API key from [ElevenLabs](https://elevenlabs.io/) or [Mistral](https://console.mistral.ai/)

**2. Build and run**

```
git clone https://github.com/user/beamer.git
cd beamer
cargo build --release
cargo run --release
```

A purple tray icon appears in your system tray. Left-click it to open the settings window.

**3. Enter your API key**

Open **Settings > API Keys** and paste your ElevenLabs or Mistral key. Keys are stored in Windows Credential Manager — never written to disk.

**4. Choose a backend**

Under **Settings > Transcription**, pick your backend:

- **ElevenLabs Scribe v2** — select a language from the dropdown
- **Voxtral Mini** — language is auto-detected, no selection needed

**5. Dictate**

Focus any text field (browser, editor, chat app), hold **Ctrl+Space**, and speak. Release the key. Your words appear in the field.

**6. Review your history**

Click the **History** tab in the sidebar to see timestamped transcriptions grouped by day. Click the copy icon on any entry to copy it to your clipboard.

---

## How-to guides

### Change the hotkey

Open **Settings > Recording**. Toggle the modifier checkboxes (Ctrl, Alt, Shift, Win) and click the key capture button to record a new key. Press Escape to cancel capture. Changes apply immediately when you save — no restart needed.

### Switch between push-to-talk and toggle mode

Under **Settings > Recording**, select a mode:

- **Push to Talk** — hold the hotkey to record, release to stop. A 400 ms tail buffer ensures the last word isn't clipped.
- **Toggle** — press once to start recording, press again to stop.

### Add vocabulary terms

Open **Settings > Vocabulary** and type a term (proper nouns, technical jargon, acronyms). Press Enter or click Add. Terms appear as tag chips and persist immediately to disk.

Vocabulary terms are sent to ElevenLabs as key terms to improve recognition accuracy. Voxtral does not use vocabulary terms.

### Change the recording glow color

Under **Settings > Appearance**, enter a hex color code. A live preview swatch shows the color. The glow border animates around the window edges while recording.

### Override the text injection method

By default Beamer tries three methods in order: UIA SetValue, SendInput, then clipboard paste. To force a specific method, edit `%APPDATA%\Beamer\config.toml`:

```toml
[injection]
preferred_method = "clipboard"  # "auto" | "uia" | "sendinput" | "clipboard"
```

### Enable auto-start on login

Set `auto_start = true` under `[appearance]` in your config file. Beamer registers itself in the Windows Run registry key on next launch.

### Run with debug logging

```
RUST_LOG=beamer=debug cargo run
```

You can also toggle the debug logging switch in **Settings > Debug** to see the last injection method used and a live status log.

---

## Reference

### Configuration

**File location:** `%APPDATA%\Beamer\config.toml`

```toml
[recording]
hotkey = "Ctrl+Space"          # Modifier+Key string
mode = "hold"                  # "hold" | "toggle"

[transcription]
backend = "elevenlabs"         # "elevenlabs" | "voxtral"
language = "en"                # ISO 639-1 code (ignored for voxtral)

[injection]
preferred_method = "auto"      # "auto" | "uia" | "sendinput" | "clipboard"
debug_logging = false

[appearance]
glow_color = "#4B0082"         # Hex color for recording border glow
overlay_enabled = true
auto_start = false             # Add to Windows startup
```

### API key storage

Keys are stored in Windows Credential Manager via the `keyring` crate:

| Service | Username |
|---------|----------|
| `beamer` | `elevenlabs_api_key` |
| `beamer` | `mistral_api_key` |

### Vocabulary file

**Location:** `%APPDATA%\Beamer\vocabulary.txt`

Plain text, one term per line, UTF-8. Maximum 100 terms (ElevenLabs limit).

### History file

**Location:** `%APPDATA%\Beamer\history.json`

JSON array of objects:

```json
[
  { "timestamp": "2026-03-02T14:30:00Z", "text": "transcribed text here" }
]
```

### Transcription backends

| Backend | Model | Protocol | Language |
|---------|-------|----------|----------|
| ElevenLabs Scribe v2 | `scribe_v2_realtime` | WebSocket, base64 PCM chunks | User-selected (ISO 639-1) |
| Voxtral Mini | `voxtral-mini-transcribe-realtime-2602` | WebSocket, raw PCM frames | Auto-detected |

Both backends receive 16-bit little-endian PCM at 16 kHz mono.

### Text injection methods

| Method | Mechanism | Best for |
|--------|-----------|----------|
| UIA SetValue | `IUIAutomationValuePattern::SetValue` | Native Win32/WPF controls, preserves undo |
| SendInput | `KEYEVENTF_UNICODE` key events | Most standard controls |
| Clipboard | Save clipboard, set text, Ctrl+V, restore | Universal fallback |

Per-app override: Warp terminal bypasses SendInput and routes directly to clipboard.

### Project structure

```
src/
  main.rs                    Entry point, single-instance guard, auto-start
  orchestrator.rs            Hotkey events -> recording -> transcription -> injection
  audio/
    mod.rs                   AudioPipeline wrapper
    capture.rs               cpal device, mono 16 kHz resampling
  transcription/
    mod.rs                   RealtimeSession, TranscriptKind enum
    elevenlabs_realtime.rs   ElevenLabs Scribe v2 WebSocket client
    voxtral_realtime.rs      Mistral Voxtral WebSocket client
  injection/
    mod.rs                   Fallback chain dispatcher
    uia.rs                   UI Automation SetValue
    sendinput.rs             Win32 SendInput
    clipboard.rs             Clipboard paste
  hotkey/mod.rs              HotkeyEvent enum
  config/
    mod.rs                   TOML config, keyring helpers
    vocabulary.rs            vocabulary.txt management
  tray/mod.rs                System tray icon and menu
  sounds.rs                  Embedded MP3 start/stop sounds
  ui/
    app.rs                   Dioxus root component
    home.rs                  Home page with status and quick settings
    history_page.rs          History view grouped by day
    settings/
      mod.rs                 Settings page layout
      recording_card.rs      Hotkey builder and mode selector
      transcription_card.rs  Backend and language selection
      api_keys_card.rs       Masked API key inputs
      vocabulary_card.rs     Vocabulary tag management
      appearance_card.rs     Glow color picker
      debug_card.rs          Debug toggle and status log
    components.rs            Card, Select, Toggle, MaskedInput, TagChip
    icons.rs                 Inline Phosphor SVG icons
    glow.rs                  Recording border glow animation
    overlay.rs               Overlay component (WIP)
    status_log.rs            In-memory log ring buffer
assets/
  styles.css                 Global styles
  icon.png / icon.ico        App icons
  startsound.mp3             Recording start sound
  endsound.mp3               Recording stop sound
```

### Build commands

| Command | Purpose |
|---------|---------|
| `cargo build` | Dev build |
| `cargo build --release` | Release build (recommended for injection testing) |
| `cargo run` | Run in dev mode |
| `RUST_LOG=beamer=debug cargo run` | Run with debug tracing |

### System requirements

- Windows 10 or 11
- Rust stable toolchain (MSVC target)
- A working microphone
- Internet connection for cloud transcription

---

## Explanation

### How the audio pipeline works

Beamer uses cpal to open the system's default input device at its native sample rate and channel count. A capture callback runs on a dedicated audio thread, converting interleaved f32 samples to mono by averaging channels. If the device sample rate differs from 16 kHz, a linear resampler downsamples on the fly, carrying fractional sample state across buffer boundaries to prevent audio discontinuities.

The resulting 16-bit little-endian PCM bytes are sent over a channel to the transcription backend, which frames them into WebSocket messages.

### Why three injection methods

Windows has no single API that works everywhere. UIA's `SetValue` is the cleanest approach — it sets text directly on the control's value pattern, preserving undo history. But many Electron and Chromium-based apps don't expose UIA value patterns. SendInput simulates keystrokes at the OS level and works broadly, but some terminal emulators (like Warp) intercept synthetic key events. The clipboard fallback is universal but destructive — it overwrites the user's clipboard temporarily.

The auto mode tries all three in order and falls back gracefully. Per-app overrides exist for known edge cases.

### How hotkey registration works

The global hotkey is registered through Dioxus's desktop shortcut handle, which wraps the `global-hotkey` crate. When settings are saved, the old shortcut is unregistered and a new one is registered with the updated key combination — no app restart required.

In push-to-talk mode, the hotkey press starts recording and the release stops it, with a 400 ms tail buffer to avoid cutting off the last spoken word. In toggle mode, successive presses alternate between start and stop states.

### Threading model

Dioxus 0.7 owns both the main thread (for the WebView2 window) and the tokio async runtime. No second tokio runtime is ever created. The orchestrator runs as an async coroutine inside Dioxus, coordinating hotkey events, audio capture, transcription, and injection.

All Win32 COM calls (UIA, SendInput, clipboard) run on `tokio::task::spawn_blocking` threads with per-thread COM initialization, because COM apartment threading requires it. Sound effects play on a separate OS thread with its own COM apartment to avoid conflicts with the WebView2 STA apartment.

### Why API keys use Windows Credential Manager

Storing API keys in plaintext config files is a common security mistake. Beamer uses the `keyring` crate with the `windows-native` backend to store keys in Windows Credential Manager, the OS-provided secure credential store. Keys never touch the filesystem and are protected by the user's Windows login credentials.

### Single-instance enforcement

Beamer creates a named Win32 kernel mutex (`Beamer_SingleInstance`) at startup. If the mutex already exists, the process exits immediately. This prevents multiple instances from fighting over the global hotkey registration and tray icon.

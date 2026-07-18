# Config Schema

## File Location

`%APPDATA%\Beamer\config.toml` (typically `C:\Users\<user>\AppData\Roaming\Beamer\config.toml`)

## TOML Schema

```toml
[recording]
hotkey = "Ctrl+Space"        # Global hotkey binding
mode = "hold"                # "hold" (hold-to-talk) or "toggle"

[transcription]
backend = "elevenlabs_batch" # elevenlabs_batch | elevenlabs_realtime | mistral_batch | mistral_realtime
language = "en"              # ISO 639-1 language code

[injection]
# Ordered fallback chain. Defaults:
#   Windows: ["sendinput", "clipboard", "uia"]
#   Linux:   ["gnome", "wtype", "ydotool", "clipboard"]
# (legacy ["ydotool", "clipboard"] chains auto-migrate to the Linux default;
#  removed backends dotool/enigo/atspi are stripped on load)
backends = ["gnome", "wtype", "ydotool", "clipboard"]
debug_logging = false        # Log injection method + target app
paste_shortcut = "auto"      # Linux clipboard backend: auto | ctrl_v | ctrl_shift_v
                             # ("auto" asks the GNOME helper which app is focused;
                             #  BEAMER_PASTE_SHORTCUT env var overrides)

[appearance]
glow_color = "#4B0082"       # Screen edge glow color (hex)
overlay_enabled = true       # Show floating transcription overlay
auto_start = false           # Start with Windows

[advanced]
vad_aggressiveness = 2       # 1-3, higher = fewer false positives
silence_timeout_ms = 600     # Silence duration to trigger speech end
pre_buffer_ms = 300          # Audio to keep before speech start
```

## Defaults

All fields have sensible defaults. Missing fields use defaults on load.
First run creates the file with all defaults.

## API Keys

**NOT stored in config.** Stored via `keyring` crate in Windows Credential Manager:
- Service: `beamer`
- Username: `elevenlabs_api_key` or `mistral_api_key`

```rust
let entry = keyring::Entry::new("beamer", "elevenlabs_api_key")?;
entry.set_password("sk-...")?;
let key = entry.get_password()?;
```

## Vocabulary File

Location: `%APPDATA%\Beamer\vocabulary.txt`
Format: one term per line, UTF-8, no trailing newline

```
Kubernetes
PostgreSQL
OAuth2
tokio::spawn
```

Max 100 terms (ElevenLabs keyterms limit).

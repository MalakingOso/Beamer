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
preferred_method = "auto"    # auto | uia | sendinput | clipboard
debug_logging = false        # Log injection method + target app

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

# Config Schema

## File Location

`%APPDATA%\Beamer\config.toml` (typically `C:\Users\<user>\AppData\Roaming\Beamer\config.toml`)

## TOML Schema

```toml
[recording]
hotkey = "Ctrl+Space"        # Global hotkey binding (dictate -> inject)
mode = "hold"                # "hold" (hold-to-talk) or "toggle"
note_hotkey = ""             # Chord that dictates into a sticky note instead of
                             # injecting. "" (the default) disables note capture
                             # entirely — no second binding is registered at all,
                             # so the dictation hotkey is unaffected.
                             # An unparseable value also yields no binding, rather
                             # than silently falling back to some other chord.
note_mode = "toggle"         # "toggle" or "hold". Toggle by default: a note is
                             # usually longer than a dictated phrase, and holding
                             # a chord through it is awkward.

[transcription]
backend = "elevenlabs_batch" # elevenlabs_batch | elevenlabs_realtime | mistral_batch | mistral_realtime
language = "en"              # ISO 639-1 language code
no_verbatim = false          # ElevenLabs only: ask the model to drop "um",
                             # "uh", false starts and stutters. Off by default
                             # because it changes what you said, not just how
                             # it is spelled. The Voxtral backends have no
                             # equivalent and ignore it, so the Settings toggle
                             # is hidden when a Voxtral backend is selected.

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

[notes]
all_workspaces = true        # Mutter only: stick() note windows so they follow
                             # you across workspaces
default_color = "purple"     # purple | violet | amber | teal | rose | slate
                             # An unrecognised value falls back to "purple"

[llm]
enabled = true                       # On-device cleanup and task extraction
base_url = "http://127.0.0.1:8080"   # The ONLY connection setting — Beamer
                                     # never spawns or configures the server
request_timeout_ms = 15000           # Generous on purpose: waking a sleeping
                                     # extraction model costs ~1.7s, or ~4s
                                     # from cold. A timeout means "not cleaned",
                                     # never "note lost"

[llm.cleanup]                        # "S1-mini" by "Superwhisper"
enabled = true
model = "s1-mini-q4_k_m"     # Server-side model id (the GGUF filename stem),
                             # NOT a path. Must match an id from GET /v1/models
styling = "semi-formal"      # casual | semi-casual | semi-formal | formal
structure = "lists"          # prose | lists
context = "general"          # general | email

[llm.extract]                # google/gemma-4-E4B-it QAT q4_0 (ladder rung 1)
enabled = true
model = "gemma-4-E4B_q4_0-it"
min_confidence = 0.5         # Below this a suggestion is not shown at all,
                             # and is not written to tasks.json either — a row
                             # nobody sees is not a labelled example.
                             # ⚠️ A backstop, not the precision mechanism.
                             # Measured, the model reports 0.90-0.98 whatever
                             # the note, so this filters ~nothing. Accept/dismiss
                             # is what makes extraction trustworthy.

[advanced]
vad_aggressiveness = 2       # 1-3, higher = fewer false positives
silence_timeout_ms = 600     # Silence duration to trigger speech end
pre_buffer_ms = 300          # Audio to keep before speech start
```

## Defaults

All fields have sensible defaults. Missing fields use defaults on load.
First run creates the file with all defaults.

## Model server

Beamer is a plain HTTP client of a **standalone** `llama-server`; it does not
spawn, configure or shut it down. That is why `[llm]` has one connection
setting and nothing about how the server runs. Launch settings live in
`deploy/llama-beamer.service`; per-model settings, including idle shutdown and
the required no-thinking flags for both models, live in
`deploy/llama-models.ini`.

Model files are **not** downloaded by Beamer. They are fetched manually into
`~/models/beamer/` and served from there.

⚠️ Never poll `GET /v1/models` on a timer. A status read resets the server's
per-model idle clock, so a background health check pins the ~3 GB extraction
model in VRAM permanently, with no error and no symptom. Beamer probes on
button press and once when the settings page opens, nowhere else.

⚠️ `[llm.cleanup]` carries a licence obligation, not just a config. `s1-mini`
is Apache 2.0 plus a binding additional term requiring the model to be
identified as `"S1-mini" by "Superwhisper"` — that exact capitalization. The
string lives in `src/llm/mod.rs` as `MODEL_CREDIT`, is rendered in the Local AI
settings card, and is pinned by an exact-equality test.

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

Renames go through `Vocabulary::rename`, which edits the term **in place**.
Don't reimplement a rename as `remove` + `add` — `add` appends, so the on-disk
order diverges from what the Vocab page shows until the next restart.

## History File

Location: `%APPDATA%\Beamer\history.json` (`~/.config/Beamer/history.json` on
Linux), written by `src/ui/history.rs`.

- **Capped at 1000 entries** (`MAX_ENTRIES`), oldest evicted first. The whole
  file is re-serialized after every injection, so an uncapped log made each
  dictation pay for every dictation before it.
- **Written atomically**: serialize to `history.json.tmp`, then rename over the
  target, so a crash mid-write can't leave a half-written file.
- **Corruption is preserved, not overwritten**: if the JSON doesn't parse,
  `load()` moves it to `history.json.corrupt` and starts empty. Silently
  defaulting would have let the next append destroy the original for good.

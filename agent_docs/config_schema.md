# Config Schema

## File Location

`%APPDATA%\Beamer\config.toml` (Windows) or `~/.config/Beamer/config.toml` (Linux)

## TOML Schema

```toml
[recording]
hotkey = "Ctrl+Space"        # Global hotkey binding (dictate -> inject)
mode = "hold"                # "hold" (hold-to-talk) or "toggle"
pause_media = false          # Pause audio/video playback when recording
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
backend = "elevenlabs"       # elevenlabs | elevenlabs_batch | voxtral | voxtral_batch
                             # (unsuffixed = realtime WebSocket; _batch = slower but
                             # usually cheaper). Language is unavailable for Voxtral.
language = "en"              # ISO 639-1 language code (ElevenLabs only)
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
pill_enabled = true          # Show the recording pill
auto_check_updates = true    # Check for app updates on launch
auto_start = false           # Launch at login

[notes]
all_workspaces = true        # Mutter only: stick() note windows so they follow
                             # you across workspaces
default_color = "random"     # random | purple | violet | amber | teal | rose | slate
                             # An unrecognised value falls back to "purple"

[llm]
enabled = true                       # Master switch for the on-device extraction pass
base_url = "http://127.0.0.1:8080"   # Beamer never spawns or configures the
                                     # server, only talks to it. Can point at a
                                     # tailnet host, e.g.
                                     # "https://callisto.taila63f23.ts.net".
                                     # An https:// URL validates against the
                                     # OS trust store with no code change,
                                     # since reqwest is built with native-tls
request_timeout_ms = 60000           # Generous on purpose. A CPU-only
                                     # extraction pass (K2-Horizon-0.9B-Q8_0,
                                     # `reasoning_effort: low`) measured
                                     # 1.3-12.4s/note on a real batch; 15s was
                                     # the tail, not headroom. A timeout means
                                     # "not analyzed", never "note lost"
connect_timeout_ms = 5000            # How long to wait for the connection
                                     # itself to open, separate from the total
                                     # request timeout above. Short on purpose:
                                     # over a tailnet, a sleeping remote
                                     # machine should fail in seconds rather
                                     # than hang for the whole generous request
                                     # timeout on every single note.
                                     # Measured RTT to a laptop over Tailscale
                                     # was 13-289ms (mdev 109, WiFi power
                                     # saving), so 5s leaves real margin.
                                     # ⚠️ Baked into the shared HTTP client's
                                     # `OnceLock` once, at process start
                                     # (`llm::client::init_http_client`, called
                                     # from `main.rs`). Changing this value
                                     # takes effect on the next restart, not
                                     # immediately. The Local AI settings card
                                     # says so.

[llm.extract]                # NANI-Nithin/K2-Horizon-0.9B-GGUF, Q8_0
enabled = true
model = "K2-Horizon-0.9B-Q8_0"
base_url = ""                # Optional override of the shared [llm] base_url,
                             # for extraction only. Empty/absent means "same
                             # server as everything else" (the common case).
                             # See agent_docs/running_on_bearcave.md.
min_confidence = 0.5         # Below this a suggestion is not shown at all,
                             # and is not written to tasks.json either — a row
                             # nobody sees is not a labelled example.
                             # ⚠️ A backstop, not the precision mechanism.
                             # Measured, the model reports 0.90-0.98 whatever
                             # the note, so this filters ~nothing. Accept/dismiss
                             # is what makes extraction trustworthy.

[sync]
url = ""                     # Sync server: wss://<tailnet-host>/sync, or empty
                             # (off). Empty means sync is disabled, the same
                             # precedent `note_hotkey` sets: a network feature
                             # must not dial out on its own. Per-machine, never
                             # synced in config.toml (a synced endpoint would
                             # reach every machine whether or not it should).
                             # See `agent_docs/sync.md` for the full story.
```

## Defaults

All fields have sensible defaults. Missing fields use defaults on load.
First run creates the file with all defaults.

## Model server

Beamer is a plain HTTP client of a **standalone** `llama-server`; it does not
spawn, configure or shut it down. That is why `[llm]` has two connection
settings (where the server is, `base_url`, and how long to wait for it to
answer, `request_timeout_ms` and `connect_timeout_ms`) and nothing about how
the server runs. Launch settings live in `deploy/llama-beamer.service`
(callisto) or the "Beamer K2-Horizon Server" Scheduled Task (bearcave);
per-model settings, including idle shutdown and the required thinking-control
flags for each model, live in `deploy/llama-models.ini` (callisto) or
`deploy/llama-models-bearcave.ini` (bearcave).

`[llm.extract]` can override `base_url` for extraction alone
(`LlmConfig::extract_base_url()` in `src/llm/mod.rs`), falling back to the
shared `[llm] base_url` when unset. An existing config that only ever set the
shared `base_url` is unaffected: extraction keeps resolving to it exactly as
before this override existed.

`base_url` moving from `127.0.0.1` to a tailnet host is why the two timeouts
are split rather than one. The far end can be asleep (a laptop, a desktop
that's suspended), and a short `connect_timeout_ms` turns that into a fast,
well-classified failure ("Server not running") instead of a multi-second stall
on every dictated note, while `request_timeout_ms` stays generous for the
actual model work once a connection exists.

Model files are **not** downloaded by Beamer. They are fetched manually into
`~/models/beamer/` and served from there.

⚠️ Never poll `GET /v1/models` on a timer. A status read resets the server's
per-model idle clock, so a background health check pins the ~3 GB extraction
model in VRAM permanently, with no error and no symptom. Beamer probes on
button press and once when the settings page opens, nowhere else.

## API Keys

**NOT stored in config.** Stored via `keyring` crate in OS credential storage:
- Service: `beamer`
- Username: `elevenlabs_api_key` or `mistral_api_key`
- Windows: Credential Manager
- Linux: Secret Service / D-Bus

```rust
let entry = keyring::Entry::new("beamer", "elevenlabs_api_key")?;
entry.set_password("sk-...")?;
let key = entry.get_password()?;
```

## Vocabulary File

Location: `%APPDATA%\Beamer\vocabulary.txt` (Windows) or `~/.config/Beamer/vocabulary.txt` (Linux)
Format: one term per line, UTF-8, no trailing newline

```
Kubernetes
PostgreSQL
OAuth2
tokio::spawn
```

Max 100 terms for batch endpoints, 50 for realtime. ElevenLabs batch applies a
20-second minimum billable duration above 100 terms; realtime's tight realtime budget is 50.

**Synced across machines**, with the notes, as a scalar at `ROOT["vocabulary"]`
in `notes.automerge` — not by copying this file. Gated on `config.sync.url`
like everything else, and last-write-wins rather than merged: two machines that
both edit while disconnected keep the document's copy and discard the other.
`src/notes/doc_vocab.rs`, and `agent_docs/sync.md`'s "The vocabulary" for why
a scalar rather than a map.

Renames go through `Vocabulary::rename`, which edits the term **in place**.
Don't reimplement a rename as `remove` + `add` — `add` appends, so the on-disk
order diverges from what the Vocab page shows until the next restart.

## History File

Location: `%APPDATA%\Beamer\history.json` (Windows) or `~/.config/Beamer/history.json` (Linux)

- **Capped at 1000 entries** (`MAX_ENTRIES`), oldest evicted first. The whole
  file is re-serialized after every injection, so an uncapped log made each
  dictation pay for every dictation before it.
- **Written atomically**: serialize to `history.json.tmp`, then rename over the
  target, so a crash mid-write can't leave a half-written file.
- **Corruption is preserved, not overwritten**: if the JSON doesn't parse,
  `load()` moves it to `history.json.corrupt` and starts empty. Silently
  defaulting would have let the next append destroy the original for good.

# Beamer

A system-tray dictation app. Press a hotkey, speak, and your words appear in
whatever text field has focus. Press a *second* hotkey and they land in a sticky
note on your desktop instead, where two on-device models tidy the transcript and
propose action items you accept or dismiss.

Beamer captures microphone audio, sends it to a cloud speech-to-text API, and
injects the transcript into the active input through a fallback chain of typing
backends. Everything after the transcript — cleanup, task extraction — runs
locally against a llama.cpp server you host yourself.

## Platform status

**Linux / GNOME on Wayland is the target that builds and runs today.** Beamer
ships a small GNOME Shell extension that handles the three things a Wayland
client is not allowed to do for itself: read the focused window's app id, type
text, and draw the recording pill.

The Windows code paths (UI Automation, `SendInput`, Credential Manager, the
low-level keyboard hook) are in the tree and build:
`XWIN_ACCEPT_LICENSE=1 cargo xwin check --target x86_64-pc-windows-msvc` is
clean. That is a cross-compile check from this Linux host, not a run. Nobody
has yet run Beamer on an actual Windows machine, so runtime behaviour (UI
Automation, WebView2, the low-level hook, notifications) is unverified. See
`todo.md` and `agent_docs/dioxus_architecture.md` for what is known and what
still needs the laptop.

There is no release pipeline yet. Build from source.

---

## Tutorials

### Getting started

From a fresh clone to your first dictation.

**1. Prerequisites**

- GNOME 48, 49 or 50 on Wayland
- [Rust toolchain](https://rustup.rs/) (stable)
- An API key from [ElevenLabs](https://elevenlabs.io/) or [Mistral](https://console.mistral.ai/)
- A working microphone

**2. Build and run**

```
git clone https://github.com/MalakingOso/Beamer.git
cd Beamer
cargo build --release
cargo run --release
```

A purple tray icon appears. Left-click it to open the main window.

**3. Install the GNOME extension**

Open **Settings → Injection**. If the Beamer Focus Helper is not installed, the
card offers to install it.

⚠️ **Wayland only rescans the extensions directory at login.** After a first
install you must **log out and back in**, then enable the extension. This is a
GNOME constraint, not a Beamer one — and it applies to every future change to
the extension too.

Verify it is live:

```
gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.GetVersion    # expect (uint32 5,)
```

Beamer still works without it — on GNOME the chain falls through `wtype` (which
GNOME refuses) to `ydotool`, and failing that to clipboard paste. You lose per-app
paste selection, the recording pill, and sticky note placement.

**4. Enter your API key**

**Settings → API Keys**. Keys go to the system keyring (GNOME Keyring via the
Secret Service on Linux, Credential Manager on Windows) — never to disk.

**5. Choose a backend**

**Settings → Transcription**:

- **ElevenLabs** — streaming, pick a language
- **ElevenLabs (Batch)** — upload after recording, more accurate on long dictation
- **Voxtral (Mistral)** — streaming, language auto-detected
- **Voxtral (Batch)** — upload after recording

**6. Dictate**

Focus any text field, hold **Ctrl+Space**, and speak. Release. Your words appear.

**7. Review your history**

The **History** page in the sidebar lists timestamped transcriptions grouped by
day. Click the copy icon on any entry to copy it.

### Dictating into a sticky note

**1. Turn note capture on**

**Settings → Recording → Note capture.** It is **off by default**, and the empty
`note_hotkey` is what turns it off — so the dictation hotkey can never be
silently diverted. The switch proposes **Ctrl+Alt+Space**; the picker below it
edits the chord. Both re-register live, no restart.

⚠️ **Do not pick a `Super+<key>` chord.** `HotkeyConfig` has no Meta field, so
the Super is silently dropped and the binding fires on the bare key. Chords
where Super *is* the trigger (`Ctrl+Super`) are safe.

**2. Speak**

Press the note hotkey. A sticky note appears on the desktop with a purple ring
on its pill, and the waveform tracks your mic. Notes are ordinary windows —
deliberately not always-on-top.

**3. Watch it tidy itself**

With the local server running (see below), the transcript is rewritten in place
by the cleanup model within about a quarter second. If the server is down the
note keeps the raw transcript — a missing server means *uncleaned*, never *lost*.

**4. Accept or dismiss the suggestions**

A slower pass proposes action items as chips on the note. Nothing reaches the
**Tasks** page unconfirmed. Dismissed suggestions are **retained**, not deleted —
they are the labelled negatives the eval corpus is built from.

---

## How-to guides

### Change a hotkey

**Settings → Recording.** Toggle the modifier checkboxes and click the key
capture button. Escape cancels capture. Changes re-register immediately.

### Switch between push-to-talk and toggle

Under **Settings → Recording**:

- **Push to Talk** — hold to record, release to stop. A 400 ms tail buffer keeps
  the last word from being clipped.
- **Toggle** — press once to start, again to stop.

Dictation and notes have independent modes; notes default to toggle, since you
rarely want to hold a key through a paragraph.

### Reorder or disable injection backends

**Settings → Injection** lists the chain in order, with each backend's
availability. Beamer tries them top to bottom until one reports success. To edit
by hand, `~/.config/Beamer/config.toml`:

```toml
[injection]
backends = ["gnome", "wtype", "ydotool", "clipboard"]
```

### Force a paste shortcut

Terminals want `Ctrl+Shift+V`; everything else wants `Ctrl+V`. On `"auto"`
Beamer asks the extension what has focus and picks per app, defaulting to
`Ctrl+Shift+V` when focus is unknown.

```toml
[injection]
paste_shortcut = "auto"   # "auto" | "ctrl_v" | "ctrl_shift_v"
```

`BEAMER_PASTE_SHORTCUT` overrides the config value. Note that the setting is
read once per session — hand-editing it mid-run takes effect at restart.

### Add vocabulary terms

The **Vocabulary** page, or **Settings → Vocabulary**. Terms persist to disk
immediately. They are sent to ElevenLabs as key terms; Voxtral does not accept
them.

### Pause media while recording

```toml
[recording]
pause_media = true
```

Beamer pauses whatever MPRIS player is playing for the duration of the recording
and resumes it after.

### Run the local model server

Beamer is a **client only** — it never spawns the server, and its entire
configuration for it is `base_url`.

```
systemctl --user enable --now llama-beamer      # deploy/llama-beamer.service
```

Or ad hoc, noting that `LD_LIBRARY_PATH` must *extend* oneAPI's rather than
replace it:

```
source ~/intel/oneapi/2025.3/oneapi-vars.sh
export LD_LIBRARY_PATH=~/Programming/llama.cpp/build-sycl/bin:$LD_LIBRARY_PATH
llama-server --models-dir ~/models/beamer \
  --models-preset ~/Programming/Beamer/deploy/llama-models.ini \
  --models-max 2 --host 127.0.0.1 --port 8080
```

**Settings → Local AI** lists both models and their states. It probes on demand
and never on a timer — a status read resets the per-model idle clock, so a
background health check would pin ~3 GB in VRAM forever.

### Measure the task extractor

```
cargo run --bin task_eval -- --limit 20
```

Grades the extraction prompt against your own accept/dismiss decisions as they
accumulate in `tasks.json`. The prompt was spot-checked against the live model,
never measured against a corpus — this is how that gets fixed.

### Run with debug logging

```
RUST_LOG=beamer=debug cargo run
```

**Settings → Debug** also shows the last injection method used and a live status
log.

---

## Reference

### Configuration

**File:** `~/.config/Beamer/config.toml` (`%APPDATA%\Beamer\config.toml` on Windows)

```toml
[recording]
hotkey = "Ctrl+Space"          # dictation chord
mode = "hold"                  # "hold" | "toggle"
pause_media = false            # pause MPRIS playback while recording
note_hotkey = ""               # empty disables note capture entirely
note_mode = "toggle"           # "hold" | "toggle"

[transcription]
backend = "elevenlabs"         # elevenlabs | elevenlabs_batch | voxtral | voxtral_batch
language = "en"                # ISO 639-1; ignored by both Voxtral backends

[injection]
backends = ["gnome", "wtype", "ydotool", "clipboard"]
paste_shortcut = "auto"        # "auto" | "ctrl_v" | "ctrl_shift_v"
debug_logging = false

[appearance]
pill_enabled = true            # recording pill drawn by the GNOME extension
auto_start = false             # launch on login
auto_check_updates = true

[notes]
all_workspaces = true          # notes stick across workspaces
default_color = "purple"

[llm]
enabled = true
base_url = "http://127.0.0.1:8080"
request_timeout_ms = 15000     # generous: waking the extraction model costs ~1.7 s

[llm.cleanup]
enabled = true
model = "s1-mini-q4_k_m"       # a server-side id, not a path — the GGUF filename stem
styling = "semi-formal"        # casual | semi-casual | semi-formal | formal
structure = "lists"            # prose | lists
context = "general"            # general | email

[llm.extract]
enabled = true
model = "gemma-4-E4B_q4_0-it"
min_confidence = 0.5           # below this a suggestion is never shown
```

Windows defaults differ for the injection chain: `["sendinput", "clipboard", "uia"]`.

### Data files

All under `~/.config/Beamer/`:

| File | Contents |
|------|----------|
| `config.toml` | The settings above |
| `history.json` | Append-only transcription log, capped |
| `notes.json` | Sticky notes — text, stage, colour, open/archived |
| `tasks.json` | Task suggestions, including dismissed ones |
| `vocabulary.txt` | One term per line, UTF-8, max 100 (ElevenLabs limit) |

### API key storage

Stored via the `keyring` crate — Secret Service on Linux, Credential Manager on
Windows. Never written to disk by Beamer.

| Service | Username |
|---------|----------|
| `beamer` | `elevenlabs_api_key` |
| `beamer` | `mistral_api_key` |

### Transcription backends

| Backend id | Model | Transport | Language |
|-----------|-------|-----------|----------|
| `elevenlabs` | `scribe_v2_realtime` | WebSocket, base64 PCM chunks | User-selected |
| `elevenlabs_batch` | `scribe_v2` | HTTPS multipart, WAV after recording | User-selected |
| `voxtral` | `voxtral-mini-transcribe-realtime-2602` | WebSocket, raw PCM frames | Auto-detected |
| `voxtral_batch` | `voxtral-mini-latest` | HTTPS multipart, WAV after recording | Auto-detected |

All four receive 16-bit little-endian PCM at 16 kHz mono.

### Injection backends

| Name | Mechanism | Notes |
|------|-----------|-------|
| `gnome` | Beamer's Shell extension, `TypeText` over D-Bus | Preferred: a real virtual keyboard inside the compositor |
| `wtype` | `zwp_virtual_keyboard_v1` | Zero-setup Unicode on wlroots compositors (Sway, Hyprland, niri) and COSMIC; GNOME and KDE refuse the protocol, so it falls through instantly there |
| `ydotool` | Kernel `uinput` device | Needs the `ydotoold` daemon and device permissions |
| `clipboard` | Save clipboard, set text, paste, restore | Universal fallback; briefly clobbers the clipboard |
| `uia` / `sendinput` | Windows-only | UI Automation `SetValue`, Win32 `SendInput` |

### GNOME Shell extension

`extension/beamer-focus@beamer.app`, version **5**, shell versions 48–50.
Interface `app.beamer.FocusProvider` at `/app/beamer/FocusProvider`:

| Method | Purpose |
|--------|---------|
| `GetVersion` | The only authoritative version check — `gnome-extensions list` reports the shell's *cached* value |
| `GetFocusedAppId` | Focused window's app id, for per-app paste selection |
| `TypeText` | Type a string via a compositor-side virtual keyboard |
| `ShowIndicator` / `HideIndicator` | Draw and clear the recording pill |
| `PlaceWindow` | Move a note window by exact title, optionally sticking it to all workspaces |
| `GetWindowFrame` | Read a window's frame rect — the verification path for `PlaceWindow` |

### Local models

Both are publisher QAT builds, in `~/models/beamer/`. Licences in `licenses/`.

| File | Size | Role |
|------|------|------|
| `s1-mini-q4_k_m.gguf` | 462 MiB | Stage 1 — transcript cleanup |
| `gemma-4-E4B_q4_0-it.gguf` | 4.80 GiB | Stage 2 — task extraction |

Measured on an Intel Arc Pro B60 (SYCL backend):

| | Measured |
|---|---|
| Cleanup of a ~60-word note | 0.225 s |
| Extraction, thinking off | 1.11 s |
| Waking the extraction model from sleep | 1.68 s |
| Cold start (spawn + 4.8 GB load) | 4.06 s |
| Resident VRAM, both models | ~4.3 GB |

⚠️ **Both models produce garbage unless explicitly told not to reason**, and both
failures look like a healthy server returning a valid response. S1-mini inherits
Qwen3's template (thinking defaults on; it was trained off) and stops after three
tokens. Gemma 4 fills `reasoning_content` while `content` stays empty, at four
times the latency for identical output. Both switches live in
`deploy/llama-models.ini`, which is their only owner — Beamer deliberately sends
no template parameters of its own.

### Project structure

```
src/
  main.rs                    Entry point, single-instance guard, auto-start
  warmup.rs                  Pre-flight credential and session checks
  media.rs                   MPRIS pause/resume around a recording
  sounds.rs                  Embedded MP3 start/stop sounds
  assets.rs                  Embedded asset extraction
  orchestrator/
    mod.rs                   Hotkey events -> recording -> transcription -> sink
    session.rs               One recording's lifetime
    sink.rs                  Where a finished transcript goes: field or note
    notify.rs                Desktop notifications
  audio/
    mod.rs                   AudioPipeline wrapper
    capture.rs               cpal device, mono 16 kHz resampling, RMS levels
  transcription/
    mod.rs                   RealtimeSession, TranscriptKind
    elevenlabs_realtime.rs   Scribe v2 streaming
    elevenlabs_batch.rs      Scribe v2 upload
    voxtral_realtime.rs      Voxtral streaming
    voxtral_batch.rs         Voxtral upload
    wav.rs                   WAV framing for the batch backends
  injection/
    mod.rs                   Fallback chain dispatcher
    gnome.rs / wtype.rs / ydotool.rs / clipboard.rs
    uia.rs / sendinput.rs    Windows backends
    focus.rs                 Focused app id, terminal detection
  hotkey/
    mod.rs                   HotkeyEvent, HotkeyConfig, chord parsing
    linux_hotkey.rs          evdev listener
    ll_hook.rs               Windows low-level keyboard hook
  notes/
    mod.rs                   Note store
    model.rs                 Note and stage types
    lifecycle.rs             Stage transitions
    pipeline.rs              The model-pass coroutine
    task.rs / task_store.rs  Task suggestions; a dismissal is data, not a delete
  llm/
    client.rs / chat.rs      llama.cpp server client
    cleanup.rs               Stage 1
    extract.rs               Stage 2
    prompts.rs               Both models' input contracts
  install/
    gnome_extension.rs       Install / enable / status for the bundled extension
  update/mod.rs              Self-update against GitHub releases
  config/
    mod.rs                   TOML config, keyring helpers
    vocabulary.rs            vocabulary.txt management
  tray/mod.rs                System tray icon and menu
  ui/
    app.rs / app_setup.rs    Dioxus root, sidebar, page routing
    home.rs                  Status and quick settings
    history_page.rs          History grouped by day
    notes_page.rs            All-notes board with search and archive
    tasks_page.rs            Accepted tasks, grouped by note
    vocab_page.rs            Vocabulary management
    sticky*.rs               Sticky note windows, chips, footer, CSS
    note_layout.rs           Pure placement function
    shell_window.rs          Note window creation
    shell_indicator.rs       Recording pill via the extension
    linux_integration.rs     Wayland/GNOME wiring
    pill.rs                  Pill state and waveform
    splash.rs                Startup splash
    settings/                One file per card section
    components.rs            Card, Select, Toggle, MaskedInput, TagChip
    icons.rs / fonts.rs      Inline Phosphor SVGs, embedded woff2
    status_log.rs            In-memory log ring buffer
  bin/
    task_eval.rs             Grade extraction against your accept/dismiss record
    ws_test.rs / voxtral_test.rs
extension/beamer-focus@beamer.app/   GNOME Shell extension
deploy/                              llama.cpp systemd unit and model preset
agent_docs/                          Deep documentation per subsystem
```

### Build commands

| Command | Purpose |
|---------|---------|
| `cargo build` | Dev build |
| `cargo build --release` | Release build (use for injection testing) |
| `cargo run` | Run |
| `cargo test` | Test suite — 250 tests |
| `RUST_LOG=beamer=debug cargo run` | Run with debug tracing |
| `cargo run --bin task_eval -- --limit 20` | Grade the extraction prompt |

---

## Explanation

### How the audio pipeline works

Beamer opens the default input device at its native rate and channel count with
cpal. A callback on a dedicated audio thread averages interleaved f32 channels to
mono; if the device rate is not 16 kHz a linear resampler downsamples on the fly,
carrying fractional sample state across buffer boundaries so buffers do not click
at their seams.

The same callback computes an RMS level for the recording pill. That level is
mapped to the bar heights through a **dB window**, not a linear gain — loudness
is perceived logarithmically, so a linear scale spends nearly its whole range on
the quietest sounds and pins every speech chunk to full scale.

The resulting 16-bit little-endian PCM is sent over a channel to the transcription
backend, which either frames it into WebSocket messages or accumulates it into a
WAV upload.

### Why a chain of injection backends

No single mechanism types text everywhere on Wayland, and the protocol is
deliberate about that: a Wayland client cannot synthesise input for another
client, because that is what a keylogger does.

The GNOME extension is the good path — code inside the Shell is not a Wayland
client, so it can drive a virtual keyboard directly. `wtype` is the good path
*elsewhere*, but it needs `zwp_virtual_keyboard_v1`, which GNOME and KDE both
refuse; its probe detects that instantly and the chain moves on. `ydotool` reaches
under Wayland entirely by writing to a kernel `uinput` device, which works but
needs a daemon and device permissions. The clipboard fallback is universal and
mildly rude — it takes the clipboard, pastes, and gives it back.

### Why Beamer ships a GNOME Shell extension

Three things a Wayland client is structurally forbidden from doing: learning
which window has focus, typing into it, and drawing an overlay at a chosen
position. All three are ordinary inside the Shell. Most apps could not justify
shipping an extension; Beamer already needed one for text injection, so the
recording pill and note placement cost two more D-Bus methods each.

Note that **GNOME extensions do not hot-reload on Wayland.** Every change needs a
full log out. Skipping it means testing stale code.

### Why notes are placed rather than remembered

Sticky notes reappear where the compositor puts them, scattered fresh around the
notes already on screen, every launch. Restart and they land *elsewhere*. This is
a decision, not a bug.

Wayland gives clients no way to learn or set absolute position, and the API that
appears to work lies: `tao::Window::outer_position()` returns `Ok((0,0))` rather
than an error, because it reads a GDK cache that has no global coordinates under
Wayland. Persisting that value persists garbage, silently. The official fix,
`xx-session-management-v1`, is not yet exposed by Mutter. So position persistence
was dropped and replaced with a pure, tested placement function. `vixalien/sticky`,
the app this was modelled on, does not persist positions either.

### Why two local models rather than one

They are different jobs. S1-mini is a 462 MiB specialist that rewrites a
transcript and cannot do anything else — it is not a chat model and cannot
extract tasks. A general model prompted to punctuate is both worse and slower at
cleanup than a model trained for it.

Extraction is a **precision** problem, not a recall problem: a task you never
asked for costs more than one you have to add by hand. The policy is strict —
first-person commitments only, aspirations excluded on purpose, every suggestion
grounded in evidence from the note. And nothing is auto-added; suggestions are
accepted or dismissed, and the dismissals are kept as labelled negatives.

### Threading model

Dioxus 0.7 owns both the main thread and the tokio runtime. No second runtime is
ever created. The orchestrator runs as an async coroutine inside Dioxus,
coordinating hotkey events, capture, transcription and injection.

Because that coroutine is polled on the main thread, a `select!` arm that
completes without awaiting will starve the entire UI — including the task that
bridges hotkey events in, which would be the only thing able to stop it. Every
`recv()` in the loop therefore handles the closed-channel `None` case explicitly.

All Win32 COM calls run on `spawn_blocking` threads with per-thread COM
initialisation. Sound playback gets its own OS thread and apartment.

### Why API keys go to the OS keyring

Plaintext keys in config files are a common and avoidable mistake. Beamer uses
the `keyring` crate against the platform's own credential store — the Secret
Service on Linux, Credential Manager on Windows — so keys never touch the
filesystem and are protected by the login session.

### Single-instance enforcement

Two instances would fight over the global hotkey and the tray icon. On Windows
Beamer creates a named kernel mutex; elsewhere it takes a lockfile whose claim is
released explicitly on quit, so a crash cannot leave a stale lock blocking
relaunch.

## Model credits

Beamer's on-device transcript cleanup uses "S1-mini" by "Superwhisper"
(https://huggingface.co/superwhisper/s1-mini), used under the Apache License
2.0 with the additional naming term recorded in `licenses/S1-mini-LICENSE.txt`.

Task extraction uses Google's Gemma 4 (`gemma-4-E4B-it`), used under the terms
in `licenses/gemma-4-LICENSE.txt`.

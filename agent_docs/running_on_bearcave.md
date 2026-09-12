# Running Beamer on bearcave

The Windows laptop. Beamer is Windows-first by design, but everything since the
sticky-notes work was built on the Linux desktop, so this was the first time
most of it ran on the machine it was written for.

✅ **Verified on real Windows hardware** (x86_64 and Windows ARM). The
first-run checklist below has been run end to end and passed — dictation
injects, note capture works, notes survive a relaunch, notifications and the
taskbar icon are correct. Treat it as a regression checklist for future
Windows changes, not an open question.

## What runs where

callisto is the always-on Linux desktop with the Arc Pro B60. If you turn
sync on, it holds the sync server. bearcave runs the app **and its own
extraction model** — see "Local extraction" below.

| Piece | Machine | Why |
|---|---|---|
| `beamer.exe` | bearcave | the app |
| llama-server, extraction (K2-Horizon-0.9B) | bearcave | 1.15 GB, CPU-only, no GPU needed (see below) |
| `sync_server` | callisto | one server, every client connects to it |

You never run `sync_server` on bearcave. Two servers means two authoritative
documents, which is the one topology the sync design tells you to avoid.

There is no cleanup pass anywhere: ElevenLabs already returns punctuated,
capitalized text, so the transcript needs no rewrite and extraction runs
against it directly. This is a change from earlier: bearcave used to run both
passes on callisto over Tailscale, then extraction moved local while cleanup
stayed remote. Cleanup is now deleted outright (see `docs/decisions.md`,
2026-09-12), so bearcave needs nothing from callisto except sync, when sync
is on. See `agent_docs/local_inference.md` for the full story.

## Linux install location

callisto runs Beamer from `~/.local/bin/beamer` + `~/.local/lib/Beamer/assets/`,
installed via `deploy/install-linux.sh` — never `/usr/bin`, which is where the
`.deb` puts it. A `.deb` install needs root to write `/usr/bin`, and
`self-replace` (the crate behind the in-app updater) creates its swap
tempfile in the same directory as the running exe before renaming over it, so
a non-root Beamer at `/usr/bin/beamer` can download an update and then fail
to install it. `~/.local/bin` is a directory `berkley` already owns.

`/usr/bin/beamer` (from an earlier `.deb` install) is intentionally left in
place, unlaunched, permanent dead weight. Do **not** `dpkg -r beamer` to
clean it up: that package also owns `/usr/bin/sync_server` and
`/usr/lib/systemd/user/beamer-sync.service`, and removing it would take down
whichever of those is actually running.

sync_server is opt-in per machine via a prompt in `deploy/install-linux.sh`,
not installed by default — only the one always-on machine (callisto) should
run it. It writes a user unit at `~/.config/systemd/user/beamer-sync.service`,
which overrides the `.deb`'s `/usr/lib/systemd/user/beamer-sync.service` of
the same name; there's no separate "old unit" to disable, just `systemctl
--user restart beamer-sync` to pick up the new binary path once installed.

⚠️ **Known gap: the self-update zip is binary-only.** A future release that
changes `assets/styles-*.css` or the icon (manganis content-hashes the
filenames) will self-update the binary but leave stale or missing files in
`~/.local/lib/Beamer/assets/` — the app would come up unstyled. Not worth
solving for a two-machine setup; re-running `deploy/install-linux.sh` from a
fresh checkout fixes it.

## Install

Grab the installer from the latest green run:

```
https://github.com/MalakingOso/Beamer/actions
```

Download **`beamer-windows-installer`**, unzip, run `Beamer_1.0.0_x64-setup.exe`.

Either build gets correct notifications now. Beamer writes its own Start Menu
shortcut carrying the AppUserModelID that Windows needs before it will attribute
a toast to an app, so the portable exe is no longer second class for that.

⚠️ **On the very first launch the shortcut is written after the window is up, so
that one session's notifications may not appear at all.** Every launch after it
is fine. If your first toast never arrives, relaunch before investigating.

The installer fetches the WebView2 runtime if it is missing, silently. On
Windows 11 it is already there and nothing happens.

## Configure

Config lives at `%APPDATA%\Beamer\config.toml`, created on first launch.

### Extraction runs on bearcave itself

No tailnet hop for inference. The default config already says so:

```toml
[llm]
base_url = "http://127.0.0.1:8080"   # bearcave's own local server
```

`[llm.extract].base_url` can still override just that one stage to a different
host (see `agent_docs/config_schema.md`), but with no cleanup pass left there
is only one stage, and the shared value is the whole story here.

Gate check before launching Beamer:

```powershell
curl.exe -s http://127.0.0.1:8080/v1/models
```

A JSON list naming `K2-Horizon-0.9B-Q8_0` is the gate. If that fails, check
`Get-ScheduledTask -TaskName "Beamer K2-Horizon Server" |
Get-ScheduledTaskInfo` and that nothing else is bound to port 8080
(`netstat -ano | findstr :8080`). Callisto only matters for sync now: if sync
is on, check the tailnet path to callisto separately.

### Local extraction — bearcave's own llama-server

Extraction needs nothing from callisto or the tailnet at all.
K2-Horizon-0.9B-Q8_0 (1.15 GB) runs CPU-only on bearcave itself, via a
different llama.cpp build than callisto's: upstream llama.cpp cannot load
K2-Horizon (no `K2HorizonForCausalLM` support), so this is built from
`MBZUAI-IFM/llama.cpp`, branch `model/K2Horizon`. See
`agent_docs/local_inference.md`'s "bearcave's extraction server" section for
why, and for the tokenizer patch that build needed.

**An NSIS installer built locally on this machine (`dx bundle --release
--package-types nsis`) now does this automatically**, on a fresh install
(no pre-existing `config.toml`) of a bundled aarch64 build:

1. `installer/k2horizon/hooks.nsh` (NSIS, `!include`d via
   `[bundle.windows.nsis].installer_hooks` in `Dioxus.toml`) embeds the 9
   runtime files plus `deploy/llama-models-bearcave.ini` and the two portable
   launcher scripts (`installer/k2horizon/start-llama-k2horizon.cmd`,
   `...-hidden.vbs`) directly into the installer — sourced from
   `vendor/llama-k2horizon/` on the build machine (gitignored; populate it by
   hand before running `dx bundle`, copying from wherever you built or
   staged the fork's `llama-server.exe` + DLLs) — and places them at
   `%LOCALAPPDATA%\Beamer\llama-k2horizon\` at install time. This is a fixed
   per-user path chosen for the same reason the installer itself is now
   `install_mode = "CurrentUser"`-only (see "Linux install location" below):
   Beamer runs `asInvoker` and can't write into `Program Files` later, so the
   runtime and the model
   (`%USERPROFILE%\models\beamer\K2-Horizon-0.9B-Q8_0.gguf`, same as before)
   both live outside the app's own install directory on purpose.
2. The same install step registers the Scheduled Task, **dormant** (no
   `/run`) — starting it before the model file exists leaves the router
   stuck reporting `"loading"` forever with no error, confirmed empirically,
   so it must never fire before the model is actually present.
3. `src/model_setup.rs` downloads the model itself on first launch (sha256-
   verified against Hugging Face's published hash, restart-from-scratch on
   failure, retried automatically on the next launch), then triggers the
   dormant task (`schtasks /run`) once verified — the Local AI settings card
   shows progress and a Cancel control while this runs. This is the one
   narrow, deliberate exception to "Beamer never touches server lifecycle":
   it starts a pre-installed task exactly once, after a download it
   initiated, never `llama-server.exe` directly.
4. Re-running the installer over an already-set-up machine is safe: the
   install step stops any running `llama-server.exe` first, `schtasks
   /create ... /F` overwrites the existing task definition rather than
   duplicating it, and `model_setup` treats a model file already present at
   the target path as something to **verify** (size + sha256), not skip
   blindly or redownload unconditionally.
5. Uninstalling removes all of it: the task, the runtime directory, and the
   model file (not the whole `models\beamer\` directory, which may hold other
   GGUFs).

The manual procedure below is now the fallback — for a from-scratch fork
build, or a non-installer setup:

1. Runtime files (9: the exe, its 4 DLL deps, 3 `ggml*.dll`,
   `libomp140.aarch64.dll`) in `C:\Users\<you>\Programming\llama.cpp-k2horizon\`.
2. The GGUF in `%USERPROFILE%\models\beamer\K2-Horizon-0.9B-Q8_0.gguf`.
3. `deploy/llama-models-bearcave.ini` (checked into the repo) as the
   `--models-preset`.
4. A Scheduled Task, **"Beamer K2-Horizon Server"**, `AtLogOn`, running
   `wscript.exe start-llama-k2horizon-hidden.vbs`, which shells out to a
   `.cmd` wrapper that launches
   `llama-server.exe --models-dir "%USERPROFILE%\models\beamer" --models-preset "...\llama-models-bearcave.ini" --models-max 1 --host 127.0.0.1 --port 8080`.
   The `wscript`/`.vbs` indirection (`WScript.Shell.Run(cmd, 0, False)`) is
   what keeps the console window hidden — Task Scheduler running the `.cmd`
   directly flashes/shows one, since window-hiding at the task-settings level
   only hides the task from Task Scheduler's own UI, not the process it
   launches. Not a Windows Service — a logon-triggered task needs no admin
   install step and is easy to inspect/restart from Task Scheduler.

Gate check, same shape as callisto's:

```powershell
curl.exe -s http://127.0.0.1:8080/v1/models
```

Expect one entry, id `K2-Horizon-0.9B-Q8_0`. If it's missing, check
`Get-ScheduledTask -TaskName "Beamer K2-Horizon Server" | Get-ScheduledTaskInfo`
and that nothing else is bound to port 8080 (`netstat -ano | findstr :8080`).

⚠️ **`--models-preset` must be honored, not merely present as a flag.**
Beamer's real request carries no sampling or template parameters of its own —
`reasoning_effort` reaches the model only if the preset's
`chat-template-kwargs` is actually applied server-side. Confirmed for this
fork build by POSTing a Beamer-shaped request (`model` + `messages` +
`response_format` only) and checking the response's leaked reasoning tag:
`<ifm|think_faster>` means `low` (the preset) took effect; `<ifm|think>` would
mean it silently fell back to the model's own default (`high`) instead.

### API keys

Keyring only, never on disk, and **they do not sync**. Enter them once here, in
Settings.

### Turn note capture on

Empty `note_hotkey` means note capture is off entirely, by design, so the
dictation hotkey can never be silently diverted. Settings, Recording, "Note
capture". It proposes **Ctrl+Alt+Space**.

⚠️ **Do not pick `Super+<key>` for anything.** `HotkeyConfig` has no Meta field,
so such a chord now fails to parse and the binding is left unbound. It used to
silently drop the Super and fire on the bare key, which was worse. Note that
`Ctrl+Super` still works, since there Super is the trigger rather than a
modifier.

### Sync, if you want it

Off unless a URL is set, deliberately. On callisto:

```bash
sync_server --config-dir ~/.config/BeamerSyncServer --bind 127.0.0.1:8081
```

Then on bearcave:

```toml
[sync]
url = "wss://callisto.taila63f23.ts.net/sync"
```

`tailscale serve` already proxies `/sync` to 8081.

⚠️ **Give the server its own `--config-dir`.** It defaults to
`BeamerSyncServer` for this reason: pointed at callisto's own Beamer config it
would be a second process writing the same document.

## Two limits to know before they surprise you

**Dictation is silently dead against elevated windows.** An unelevated
`WH_KEYBOARD_LL` hook receives no input destined for a higher-integrity window,
and `SendInput` into one is blocked. Beamer requests `asInvoker`, so dictating
into an admin PowerShell does nothing at all: no error, no notification. There
is no fix short of a signed `uiAccess="true"` binary in Program Files.

**Rolling back to an older build looks like data loss and is not.** Run this
build once and `notes.json` is rewritten in the new shape. An older build cannot
parse that, quarantines it to `notes.json.corrupt`, and comes up with an empty
board. Your notes are fine: the old build knows nothing about `sync\`, so
`notes.automerge` is untouched and coming back to this build restores
everything. Only notes made during the rollback session are lost.

## First-run checklist

**Run end to end on real Windows hardware and passed.** Kept in order as a
regression checklist for future Windows changes. The first item is the
regression that matters most, and the second is the one that would silently
not exist if the Windows hotkey work had been done as a four-line patch.

1. **Dictation hotkey injects into a focused field.** Notepad first, then
   something with a real UI Automation surface.
2. **Note hotkey makes a sticky note appear.**
3. **Restore five or more notes at once.** Close Beamer with several notes open,
   relaunch. Watch for `WebView2Error(HRESULT(0x8007139F))` or a panic. Window
   opens are serialized specifically to avoid this, and this is the test.
4. **Notes are not placed under the taskbar**, and no 40 px strip is wasted at
   the top of the screen.
5. **Click a link chip whose URL contains `&`.** The browser should get the
   whole URL and no console window should flash.
6. **Right-click `beamer.exe`, Properties.** The icon should be there. CI
   already asserts the manifest, so this is belt and braces.
7. **Notifications say Beamer, not PowerShell**, and carry Beamer's name in
   the action centre. Relaunch once first, see the warning above. If they say
   PowerShell, something is stale, because that code path is gone. If they do
   not appear at all, check
   `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Beamer.lnk` exists.
8. **Dictate into an elevated window and confirm nothing happens.** Expected,
   and worth seeing once so you recognise it later.
9. **With the local server reachable, dictate a note.** It should grow task
   chips, with no red footer label.
10. **Stop bearcave's own extraction server**
    (`Stop-ScheduledTask -TaskName "Beamer K2-Horizon Server"`, and the process
    it launched keeps running even once the task itself shows Ready again, so
    also `Get-Process llama-server | Stop-Process`). Dictate three notes:
    extraction fails with its red footer label, no task chips. This is the
    regression test for that property, not just a config check.
11. **Start the task again** (`Start-ScheduledTask`) **and dictate a fourth.**
    Extraction runs, **and the three stale notes analyse themselves with no
    click.** That is the backlog sweep. Confirm a still-failing note can also
    be retried from the footer.
12. **Fresh-install download.** On a machine with no `config.toml` and no
    model file, launch a bundled aarch64 build: the Local AI settings card
    should show download progress unprompted, a Cancel control while it
    runs, and the server should start (task goes `Running`, `GET
    127.0.0.1:8080/v1/models` succeeds) once it completes — a dictated note
    then extracts correctly with no manual setup at all.
13. **Cancel and retry.** Cancel a download mid-flight from the card; confirm
    the `.part` file is gone and the card returns to idle. Relaunch: the
    download should restart from scratch (not resume) automatically.
14. **Installer re-run over an already-set-up machine.** With bearcave's own
    manual setup (or a prior install) already in place and its Scheduled Task
    running, run the newly-built installer again. Confirm: the old
    `llama-server.exe` process is gone afterward (not orphaned holding port
    8080), the task is updated rather than duplicated (`schtasks /query /tn
    "Beamer K2-Horizon Server"` shows exactly one), and the already-present
    model is verified rather than redundantly redownloaded (no download
    progress shown in the card on next launch).
15. **Uninstall.** Confirm the task, `%LOCALAPPDATA%\Beamer\llama-k2horizon\`,
    and the model file are all gone; `models\beamer\` itself (and any other
    GGUF in it) is left alone.

## Things that are known-unverified on Windows

Not bugs, just untested, so do not spend time being surprised by them.

- Mixed-DPI multi-monitor note placement. Uniform scale round-trips correctly,
  so one display or two matched ones are fine.
- Ctrl+F, F5 and Ctrl+P are disabled inside note windows. Editing shortcuts
  should still work; worth confirming in a note textarea.
- Each note is its own WebView2 process set. Watch `msedgewebview2.exe` memory
  with several notes open, since Windows scales worse here than WebKitGTK does.
- Whether the tray icon survives an explorer restart
  (`taskkill /f /im explorer.exe`).

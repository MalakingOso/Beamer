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

callisto runs Beamer from `~/.local/bin/beamer` (the binary is the whole app),
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

The self-update zip is binary-only, and that's complete: the stylesheet,
fonts, icon and GNOME extension are compiled in (1.0.4 shipped the opposite
and came up unstyled after an exe-only update). Re-running
`deploy/install-linux.sh` also deletes the old `~/.local/lib/Beamer/assets/`
and `~/.local/share/beamer/extension/` copies earlier installs left behind.

## Install

Grab a build from the [latest release](https://github.com/MalakingOso/Beamer/releases/latest)
(CI only runs on manual dispatch, so the Actions page is the fallback for
unreleased builds, not the source):

- **x64 PCs:** the `Beamer_<version>_x64-setup.exe` installer.
- **bearcave (ARM64):** the `aarch64` zip — unzip it and run `beamer.exe`.

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

**Beamer installs and updates all of this itself, on launch**
(`src/components/`), on a Windows ARM64 build, however the exe got there
(installer, self-update, hand copy):

1. The runtime (the 9 files below) is downloaded silently from a GitHub
   **prerelease** (`llama-runtime-k2h-N`, published with
   `deploy/publish-llama-runtime.sh`), sha256-verified, and swapped into
   `%LOCALAPPDATA%\Beamer\llama-k2horizon\`. The preset
   (`deploy/llama-models-bearcave.ini`) and the two launchers
   (`installer/k2horizon/start-llama-k2horizon.cmd`, `...-hidden.vbs`) are
   embedded in the exe and written into the same dir. This is a fixed
   per-user path, not the app's install dir: Beamer runs `asInvoker` (see
   "Linux install location" above for the same reasoning), so the runtime and
   the model (`%USERPROFILE%\models\beamer\K2-Horizon-0.9B-Q8_0.gguf`) both
   live somewhere it can always write.
2. The Scheduled Task is registered (`schtasks /create /xml ... /f`, so it's
   updated, never duplicated) from XML the exe renders. Its logon trigger
   names the current user: an any-user `<LogonTrigger>` is admin-only, and the
   unelevated app gets "Access is denied" for it (1.0.5's first build did).
3. The model (1.15 GB) downloads on its own on a **fresh install** (no
   `config.toml` before that launch). On an existing install it's offered in
   Settings → Updates → Components with a Download button, progress, Cancel
   and Retry. A model already on disk is **verified** (size + sha256) once,
   never redownloaded blindly, and never prompted for.
4. All of it is one all-or-nothing group. Nothing live is touched until every
   out-of-date part is downloaded and verified; then the task is ended and
   `llama-server.exe` killed (its files are locked while it runs), everything
   is applied, and the task is run. **The server is never started before the
   model exists**: a router started without its model sits at `"loading"`
   forever with no error, confirmed empirically. If a part can't be staged
   (say the model is waiting on its Download click), the server keeps
   running on its current files. This is the one narrow, deliberate
   exception to "Beamer never touches server lifecycle": only this apply
   step ends, registers and runs the task, and never `llama-server.exe`
   directly.
5. `%APPDATA%\Beamer\components.json` records what was applied, so later
   launches only compare that record and check the files exist, with no
   re-hashing. Delete it (and the runtime dir) to force a full reinstall.
6. The installer (`installer/k2horizon/hooks.nsh`) now only uninstalls: the
   task, the runtime dir and the model file (not the whole `models\beamer\`
   directory, which may hold other GGUFs).

**First launch after upgrading from 1.0.x is a one-time migration, not a
bug:** there's no `components.json` yet, so the runtime re-downloads (~8 MB),
the existing model gets one full verify (a few seconds), and the server
restarts once. No model prompt, since the file verifies.

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

**Rolling back to an older build looks like data loss and is not.** The corpus
lives in `sync\notes.automerge`, which an older build knows nothing about, so
coming back to this build restores everything: a document on disk is never
re-seeded from the `notes.json` mirror. The price is everything the rollback
session did — notes made then, and edits to older notes, exist only in the
mirror the old build wrote, which this build will never read again.

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
5. **Export a dated task to the calendar.** The `.ics` should open externally
   with no console window flashing (`ui::open_external` goes through
   `ShellExecuteW`, not a shell — this is the regression test for the
   command-injection fix).
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
13. **Cancel and retry.** Cancel a download mid-flight; confirm the `.part`
    file is gone and the row returns to Pending with a Download button that
    restarts it from scratch (not resume). Leaving Settings mid-download must
    not stop it.
14. **Upgrade over 1.0.x.** With an older install's runtime, task and model in
    place, launch the new build: expect the one-time migration (runtime
    re-download, one model verify, one server restart) and no model prompt.
    Afterward the old `llama-server.exe` is gone (not orphaned holding port
    8080), `schtasks /query /tn "Beamer K2-Horizon Server"` shows exactly one
    task, and `components.json` lists every component. A second launch
    touches nothing.
15. **Group gate.** Remove the model and don't accept its download, then
    launch a build whose embedded ini differs: the runtime dir must be
    unchanged and the server not stopped until Download is clicked and
    finishes.
16. **Uninstall.** Confirm the task, `%LOCALAPPDATA%\Beamer\llama-k2horizon\`,
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

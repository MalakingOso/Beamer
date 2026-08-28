# Running Beamer on bearcave

The Windows laptop. Beamer is Windows-first by design, but everything since the
sticky-notes work was built on the Linux desktop, so this is the first time most
of it runs on the machine it was written for.

⚠️ **Nothing below has been run on Windows.** CI proves the code compiles, the
tests pass and the installer builds. It has never launched the app, pressed a
hotkey, injected text or opened a note window. Treat the checklist at the end as
the actual test, and expect to find things.

## What runs where

callisto is the always-on Linux desktop with the Arc Pro B60. It holds the
models and, if you turn sync on, the sync server. bearcave runs Beamer and
nothing else.

| Piece | Machine | Why |
|---|---|---|
| `beamer.exe` | bearcave | the app |
| llama-server | callisto | the B60 is there, and models are 4.3 GB resident |
| `sync_server` | callisto | one server, every client connects to it |

You never run `sync_server` on bearcave. Two servers means two authoritative
documents, which is the one topology the sync design tells you to avoid.

## Install

Grab the installer from the latest green run:

```
https://github.com/MalakingOso/Beamer/actions
```

Download **`beamer-windows-installer`**, unzip, run `Beamer_1.0.0_x64-setup.exe`.

**Use the installer rather than `beamer-windows-portable`.** The Start Menu
shortcut it creates carries a real AppUserModelID, and that is the only thing
that stops Windows branding Beamer's notifications "PowerShell". The portable
exe is for a quick look, not for living with.

The installer fetches the WebView2 runtime if it is missing, silently. On
Windows 11 it is already there and nothing happens.

## Configure

Config lives at `%APPDATA%\Beamer\config.toml`, created on first launch.

### Point it at callisto's models

```toml
[llm]
base_url = "https://callisto.taila63f23.ts.net"
```

That is Tailscale Serve in front of a loopback llama-server. `reqwest` is built
with `native-tls`, so the certificate validates against the Windows trust store
with no extra work.

Check it from bearcave before launching Beamer, because a failure here is a
tailnet problem and not an app problem:

```powershell
curl.exe -s https://callisto.taila63f23.ts.net/v1/models
```

A JSON list naming both models is the gate. If that fails, make sure callisto is
awake, `systemctl --user is-active llama-beamer` says active, and
`tailscale status` on bearcave shows callisto.

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

⚠️ **Attachments do not sync yet.** A note crosses, its image does not, and
bearcave shows the missing-file card. This is the top open question in
`agent_docs/sync.md`.

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

In order. The first is the regression that matters most, and the second is the
one that would silently not exist if the Windows hotkey work had been done as a
four-line patch.

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
7. **Notifications say Beamer, not PowerShell.** Only true if you used the
   installer.
8. **Dictate into an elevated window and confirm nothing happens.** Expected,
   and worth seeing once so you recognise it later.
9. **With callisto reachable, dictate a note.** It should clean itself and grow
   task chips.
10. **Stop llama-server on callisto** (`systemctl --user stop llama-beamer`,
    never `pkill -f`, which matches the shell running it). Dictate three notes.
    All three should be captured and show the red footer label.
11. **Start it again and dictate a fourth.** It cleans, **and the three stale
    notes clean themselves with no click.** That is the backlog sweep.

## Things that are known-unverified on Windows

Not bugs, just untested, so do not spend time being surprised by them.

- Mixed-DPI multi-monitor note placement. Uniform scale round-trips correctly,
  so one display or two matched ones are fine.
- Dragging a URL from a browser onto a note does nothing. wry disables HTML5
  drag-and-drop on Windows and the synthesized event carries no `text/uri-list`.
  Dropping a file works. The paperclip button works.
- The drop-target highlight never appears, because `dragenter` is never
  synthesized.
- Ctrl+F, F5 and Ctrl+P are disabled inside note windows. Editing shortcuts
  should still work; worth confirming in a note textarea.
- Each note is its own WebView2 process set. Watch `msedgewebview2.exe` memory
  with several notes open, since Windows scales worse here than WebKitGTK does.
- Whether the tray icon survives an explorer restart
  (`taskkill /f /im explorer.exe`).

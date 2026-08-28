# Live sync: `sync_server`, `notes::sync_client`, and what does not sync

Task 10 wires the automerge document from `notes::sync_doc` to a second
machine over a WebSocket. Read `agent_docs/sticky_notes.md` and
`agent_docs/local_inference.md` first if you have not; this document assumes
the automerge corpus (`SyncDoc`, `flush::flush_stores`, the genesis change)
already exists, since Task 10 built the network layer on top of it, not the
document itself.

## Layout

```
%APPDATA%\Beamer\sync\   (or ~/.config/Beamer/sync/)
    notes.automerge          shared note and task corpus
    attachments/<hash>.<ext> content-addressed bytes
%APPDATA%\Beamer\            <- machine-local, never synced
    config.toml              machine-specific
    machine.json             pos / size / open, per machine
    history.json             dictation history
```

`notes.automerge` under `sync/` is the same on every machine, given time and
a server; that is the whole point of this task. `sync/attachments` is carried
by a different mechanism entirely, Syncthing, not by this protocol: see
"Attachments: Syncthing carries the bytes" below for what that means and why.
Everything directly under the config root is machine-local by design, and
neither `sync_client` nor Syncthing touches any of it: `sync_client` only
reads `config.sync.url`, and Syncthing is pointed at `sync/attachments`
specifically, never at the config root or at `sync/` as a whole.

## Why automerge, not last-write-wins

A note is edited on whichever machine is open, offline, for however long. A
plain "newest write wins" sync (an mtime check, a version counter) throws away
one machine's edit whenever both machines touched the same note while
disconnected, which for a background dictation app is the common case, not
the edge case. `automerge::sync` merges instead: two machines that both
typed into a note's body during the same offline stretch keep both edits,
spliced by position, because `Note.body` is stored as an automerge `Text`
object and not a string register. `sync_doc::put_text`'s doc comment has the
detail; this document is about what moves the merged result between machines,
not the merge itself.

## Why a self-hosted socket on the tailnet, not an account

The alternative to `sync_server` is a hosted relay: sign in, and some service
keeps your notes converged for you. Two reasons that was never the plan here:

- **The tailnet already authenticates these devices.** Every machine on
  `berkley`'s tailnet is already an identity `tailscaled` vouches for. An
  account system would be a second authentication scheme layered on top of
  one that already answers the only question that matters, whether this is
  one of the user's own machines, for free.
- **An account puts the notes on someone else's server.** Beamer's notes
  include dictated task lists and, per `agent_docs/local_inference.md`,
  whatever a local model extracted from them. A relay operator is a party
  with plaintext access to that, permanently, for a feature whose entire job
  is running two machines you already own and already trust each other.

`sync_server` is the plainer trade: it runs beside `llama-server` on
`callisto`, reachable at `wss://callisto.taila63f23.ts.net/sync` through
`tailscale serve`'s proxy, on the same trust boundary `llama-server` already
uses (see `agent_docs/local_inference.md`'s note on `base_url`). No account,
no separate credential, no server outside the tailnet ever sees a note.

## Why loopback only, and why Funnel is never an option

`sync_server` binds `127.0.0.1:8081` and refuses to start on anything else
(`ensure_loopback` in `src/bin/sync_server.rs`). The sync protocol itself
carries **no authentication**: a WebSocket that reaches the port gets to
read and write the whole corpus. That is fine only because `tailscale serve`
is the thing deciding who reaches the port. It proxies `/sync` from the
tailnet's HTTPS listener to loopback, so the only way in is already a
tailnet-authenticated connection. Binding `0.0.0.0` would skip that check
entirely and hand the corpus to anything that can route to the host.
Tailscale **Funnel**, the mode that exposes a `tailscale serve` proxy to the
public internet, is categorically off the table for the same reason
`agent_docs/local_inference.md` rules it out for `llama-server`: it would
turn "reachable by my tailnet" into "reachable by anyone," on a protocol that
was never given a password to check.

## The server's `--config-dir`, and running it beside a Beamer install

`sync_server` runs on `callisto`, and `callisto` is also a machine a
Beamer install can run on directly, at least for testing this feature.
`sync_server`'s default `--config-dir` is deliberately **not**
`Config::config_dir()` (the `Beamer` leaf a real Beamer install uses) for
exactly that reason: defaulting to the same directory would make every
plain `cargo run --bin sync_server`, with no flags, a silent file-sharing
arrangement between two independent processes writing the same
`notes.automerge`. The default is a distinct directory
(`BeamerSyncServer`, alongside `Beamer` under the OS config root) so that
sharing has to be asked for.

Two processes writing the same document file is safe: `SyncDoc::save`'s temp
file name is scoped by pid (`notes.automerge.tmp.<pid>`), so two writers can
no longer collide on one temp path or have a `rename` fail because the other
process already moved the file it was racing. They still race the final
`rename` itself, one save wins and the other is superseded, but neither can
lose data by it: the loser's own in-memory `AutoCommit` still holds its
changes and reconciles them again on its own next tick, same as an ordinary
merge from a slightly-stale file always has.

If you deliberately want `sync_server` to serve the exact corpus a co-located
Beamer install reads and writes locally, point it there on purpose:

```
sync_server --config-dir "$HOME/.config/Beamer" --bind 127.0.0.1:8081
```

Otherwise, leave `--config-dir` unset and the server keeps its own corpus,
under `BeamerSyncServer`, entirely separate from any Beamer install that
happens to run on the same box.

## The genesis change

Every automerge document, client or server, starts from the same 146-byte
genesis change (`src/notes/genesis.automerge`, loaded by `sync_doc::new_document`)
before anything else touches it. `sync_server` gets this for free: it opens
its document the same way a client does, through `SyncDoc::open`, which calls
`new_document` whenever there is nothing on disk yet. There is no separate
"server first-boot" code path that could drift from the client's.

Why this matters enough to test: two documents created independently, each
with `AutoCommit::new()`, hold two *different* root maps at the same logical
key, because each one's root map carries the actor id that created it. A
merge between them does not blend those maps: one wins outright and the
other's entire contents become unreachable, deterministically the same side
every time. Measured directly, before genesis existed: merges shaped like
`two_documents_seeded_independently_both_keep_their_notes_after_a_merge` lost
one side's whole corpus in **200 of 200 runs**. Starting every document from
the same first change gives every root map the same object id everywhere, so
two documents write into the *same* map and merge per note instead of
conflicting whole. `sync_server`'s own test,
`a_fresh_server_document_starts_from_the_same_genesis_a_client_would`, pins
the server side of that fact directly.

## API keys

Never synced, never in `notes.automerge`, never on disk anywhere. Keys live
only in the OS keyring (`keyring` crate: Windows Credential Manager, or the
Secret Service on Linux) and are entered once, by hand, on each machine. A
second machine does not inherit the first machine's ElevenLabs or Mistral key
through sync; there is no mechanism that would even let it, since the sync
protocol only ever touches `notes.automerge`.

## Why `config.toml` must never sync

`config.toml` sits next to `sync/`, not inside it, and Task 10 does not
change that. Four fields make the case on their own, each a different flavor
of "synced wrong, this actively hurts you":

- **`injection.backends`** is OS-branched, with no validation that a stored
  backend name is even legal for the machine running it. A Linux machine's
  chain (`gnome`, `wtype`, `ydotool`, `clipboard`, and so on) synced onto
  Windows would name backends that do not exist there, or skip the Windows
  ones the receiving machine actually needs.
- **`appearance.auto_start`** is applied at every launch. Syncing `true` from
  one machine silently enrolls the second machine in launching Beamer at
  login too, a behavior change with no note in the UI and no way to trace
  where it came from.
- **`llm.base_url`** is per-machine by definition: it names *this* machine's
  local inference server (or a tailnet URL to one). Syncing it would point
  every machine at whichever one last wrote the file.
- **`notes.all_workspaces`** and **`injection.paste_shortcut`** are
  Mutter/Linux-only settings. Syncing either onto Windows writes a field that
  means nothing there and, on the next sync back, could clobber whatever the
  Linux machine had set.

`config.sync.url` itself is in this same file for the same reason: which
server *this* machine dials is exactly as per-machine as `llm.base_url`. See
`src/config/mod.rs`'s `SyncConfig` doc comment.

## What this protocol does *not* carry

**Attachments never move through `automerge::sync`.** `notes.automerge`
carries a note's *reference* to an attachment, the content hash and
extension `Attachment` stores, but never the bytes themselves.
`automerge::sync` moves changes to the document; it was never given a
channel for the separate `attachments/<hash>.<ext>` files those changes
point at, and it never will be, by design, not by omission: the document is
kept small and the two kinds of data are carried by two different
mechanisms suited to each. See "Attachments: Syncthing carries the bytes"
below for what actually moves the bytes, and how far that closes the gap
this section used to describe as unsolved.

## Attachments: Syncthing carries the bytes

The decision: attachment bytes are not this protocol's problem to solve.
`notes.automerge` keeps carrying the document, exactly as everything above
this section describes, and a second, unrelated piece of software,
Syncthing, is pointed at `<config_dir>/sync/attachments` and keeps that one
directory's contents the same across machines. Two different jobs, two
different tools, on purpose: automerge merges a document machines edit
concurrently, which attachment bytes are never edited, only added, so
merging is not a problem they have.

⚠️ **Not installed on either machine yet.** Everything below is setup
instructions to follow, not a description of something already running.
Nothing here has been exercised end to end.

### What Syncthing carries, and what it must never carry

Syncthing's one job is the `attachments` folder and nothing else:

```
<config_dir>/sync/attachments/<hash>.<ext>   <- Syncthing's folder, only this
<config_dir>/sync/notes.automerge            <- automerge::sync's job, not Syncthing's
<config_dir>/machine.json                    <- machine-local, never synced by anything
<config_dir>/config.toml                     <- machine-local, never synced by anything
<config_dir>/notes.json                      <- a derived export, never read back, not worth syncing
<config_dir>/tasks.json                      <- same, derived from the document
WebView2 profile (Windows, outside config_dir) <- browser engine state, never synced
```

Pointing Syncthing at `sync/` itself instead of `sync/attachments` would
also try to sync `notes.automerge`, which already has its own live sync
protocol above. Two mechanisms writing the same file, on their own
schedules, with no coordination between them, is exactly the two-writers
hazard "The server's `--config-dir`" above works around for two `sync_server`
processes sharing one document, except Syncthing has no equivalent of that
section's pid-scoped temp file or its "loser reconciles again next tick"
guarantee. Worse, Syncthing's own answer to two conflicting versions of a
file is a `.sync-conflict-<date>-<time>` copy sitting next to the original,
which is meaningless for an automerge document: nothing reads a
`notes.automerge.sync-conflict-...` file, so a conflict copy there is not a
second chance to recover data, it is a dead file that silently never gets
merged in. Point Syncthing at `attachments/` only.

### Why the attachments folder is safe to sync, when the rest is not

Attachment bytes are the one thing under `config_dir` that fits a plain
file-sync tool without any of the caveats the rest of this document spends
so much space on:

- **Content-addressed and immutable.** A file's name *is* its sha256 hash
  (`<hash>.<ext>`, see `model::owned_file_name`). Two machines can never
  disagree about what a given filename should contain, because the filename
  only exists in the first place because of what the bytes hash to. There is
  no "which version is newer" question for Syncthing to get wrong, because
  there is only ever one possible version of `<hash>.<ext>` that could exist
  under that name.
- **Write-once.** A hash file is created once, by `edit::adopt_into`'s
  copy-then-rename, and is never edited in place afterward. Syncthing's
  conflict-copy machinery exists for files that change; nothing here changes.
- **No cross-file relationships Syncthing needs to preserve.** Each file
  stands alone; nothing about `<hash-a>.png` depends on whether
  `<hash-b>.jpg` has arrived yet.

That last point is also the thing to expect and not be alarmed by: **a note
can arrive before its image does.** The document propagates over
`sync_client`'s WebSocket, typically in milliseconds; the bytes propagate
over Syncthing, on its own schedule, over however Syncthing and the two
machines' networks are getting along at that moment. A note showing up with
a missing-file card that fills in a few seconds (or longer, on a slow link,
for a large attachment) later is the expected shape of this design, not a
bug to chase.

### The deletion policy, and why it trades disk for safety

Content addressing makes attachment bytes safe to sync; it does not make them
safe to *delete* the way `edit::release_attachment_bytes` used to.
Refcounting there only ever looks at the local store: it removes
`<hash>.<ext>` once nothing in *that machine's* notes references it. That was
correct when attachments never left the machine that dropped them. It stops
being correct the moment Syncthing can carry the same file to a second
machine, because "nothing local references this" says nothing about whether
a note open on that second machine still does. Machine B deleting its last
local reference to a hash, with Syncthing propagating that deletion the same
way it propagates a new file, would silently take the bytes with it on
machine A too, permanently, the moment sync brings the two machines' folders
back in step.

`NoteStore.sync_enabled` is the fix: with a sync server configured
(`config.sync.url` non-empty, the same signal `sync_client::should_start`
already reads), `release_attachment_bytes` still runs its refcount check, but
once that check says "nothing local references this any more," it stops
there instead of deleting the file. An orphaned file left on disk costs disk
space, recoverable and cheap to clean up later once something exists that
can actually check references across every machine, rather than only the
local one. A referenced image deleted out from under a note on every machine at once is
not recoverable by anything. That asymmetry is the whole argument: given a
choice between wasting some disk and losing a photo permanently, waste the
disk. With sync off, `release_attachment_bytes` behaves exactly as it always
has: local refcounting, real deletion once nothing local points at a hash.

This closes neither of the two scenarios `todo.md` already describes for
cross-machine attachment deletion (a still-referenced hash going missing
because it was deleted on a machine that had no reference to it yet, and a
machine that never held a "user's original" losing the only copy that ever
existed there); a real fix for those needs an actual cross-machine notion of
"referenced nowhere," which nothing here builds. What this policy does is
narrower and unconditional: it stops Beamer's own local refcount, on its
own, from being the thing that deletes a still-needed file. See `todo.md`'s
sync section for what is still open.

### Ignore Delete: a second, independent layer

Syncthing has its own per-folder advanced setting, **Ignore Delete**, that
stops a deletion on one side from propagating to the other at all: the file
disappears locally but Syncthing does not remove it from peers, and does not
recreate it if the peer's own copy is later deleted too. Turning this on for
the `attachments` folder, on both machines, is worth doing independently of
whatever Beamer's own `sync_enabled` policy does above: it is a second,
unrelated layer, enforced by Syncthing itself rather than by Beamer's code,
and it protects against exactly the same failure mode from the other
direction, a deletion propagating when it should not have. Recommended, not
required; Beamer's own deletion policy does not depend on it being set.

### Setup, on both machines

Nothing below has been run. These are the steps to follow, not a record of
what is already configured.

**On the Linux desktop (the machine `sync_server` also runs on):**

1. `sudo apt install syncthing`
2. Start it (`systemctl --user enable --now syncthing`, or run it once by
   hand) and open its web UI, `http://127.0.0.1:8384` by default.
3. Add a folder pointed at `~/.config/Beamer/sync/attachments`. Give the
   folder id something recognizable, e.g. `beamer-attachments`; the label
   only has to match on both machines if you want the UI to line up.
4. Under that folder's **Advanced** settings, turn on **Ignore Delete**.
5. Under **Actions → Settings → GUI**, note the device ID (or read it from
   `syncthing --device-id`) for pairing in step 3 on the laptop.

**On the Windows laptop:**

1. Install Syncthing from the official Windows installer
   (syncthing.net/downloads), or `winget install Syncthing.Syncthing`.
2. Open its web UI and add the desktop as a remote device, addressed by its
   tailnet hostname (the same `*.ts.net` address `sync_server`'s own
   `wss://…/sync` endpoint uses, so the tailnet is already doing the
   authentication work here too, the same case "Why a self-hosted socket on
   the tailnet, not an account" makes above) rather than a bare IP.
3. Accept the desktop's offered `beamer-attachments` folder (or add the same
   folder id manually if auto-accept is off), pointed at
   `%APPDATA%\Beamer\sync\attachments` on this machine.
4. Turn on **Ignore Delete** on this side too. It is a per-folder,
   per-device setting; both machines need it set for the protection to hold
   in both directions.

Once both sides show the folder as up to date, an attachment dropped on
either machine should appear in the other's `attachments` directory within
whatever interval Syncthing's file watcher and the tailnet link allow, no
different from any other pair of machines running Syncthing between two
folders. That behavior has not been checked by hand yet; do so before
relying on it.

## Threading: the socket task never writes anything Dioxus owns

`notes::sync_client::run_client` is the live-sync coroutine, started once
from `App()` via `use_sync_client`, a no-op when `config.sync.url` is empty
(`should_start`, the same empty-means-off precedent as `note_hotkey`).

The actual WebSocket lives on a plain `tokio::spawn` task
(`sync_client::socket_task`) that only ever moves bytes between the socket
and two channels. It holds no `Signal`. Applying an incoming change writes
`Signal<NoteStore>`/`Signal<TaskStore>`, and Dioxus desktop's signal arena is
thread-local, so that step (`apply_incoming`) runs on the Dioxus coroutine
instead. See `agent_docs/dioxus_architecture.md` for the general rule this
follows.

`apply_incoming` also never writes `notes.automerge` to disk. It mutates the
in-memory document and calls `SyncDoc::mark_pending_save`; the existing
500ms tick in `notes::flush` is still the only place `SyncDoc::save` is
called. `flush::run_document_pass`'s own before/after heads comparison cannot
see a mutation that already happened before that tick started, which is
exactly what `mark_pending_save`/`has_pending_save` exist to catch. See the
doc comment on `SyncDoc`'s `pending_save` field for the detail.

`sync_server` has no such split: it is not a Dioxus process, so
`receive_sync_message` and `SyncDoc::save` run inline, in the same task that
read the message off the socket.

## Offline-first

A dropped or failed connection is expected, not exceptional. `run_client`
reconnects with backoff (1s, doubling, capped at 30s) forever, and a
connection failure never surfaces as more than a debug log line. Notes work
fully with no server reachable: the document on disk is each machine's own
source of truth regardless of whether a peer or a server ever sees it. Losing
the server loses live propagation between machines and nothing else.

## Manual verification

`sync_server` and `sync_client` were checked end-to-end twice, by hand, on
loopback, with throwaway scripts (neither committed).

The first drove three clients that each opened a fresh connection, did one
thing, and disconnected: a client wrote `hello=from-a` and disconnected, a
second, freshly-connected client synced (saw `from-a`) and wrote
`world=from-b`, and a third, freshly-connected client synced and saw both
values. `notes.automerge` under the test config directory held both writes on
disk afterward. This proved the wire protocol and the server's persistence,
but every operation in it predated its own connection, so it could not have
caught a client that only pushes what it had *at connect time* and goes silent
on anything written while already connected, which is exactly the bug the
first version of `sync_client`'s idle-push interval had.

The second check was built to catch that: two clients connect and stay
connected, converge to idle, and only then does one of them write. B saw A's
write without either side ever reconnecting, confirming the periodic
`send_pending` check inside `connect_and_sync` (see "Threading" above) is
what makes propagation live, and not something that only happens on the next
reconnect.

Both checks are single-machine: the wire protocol and the server's behavior,
not two real machines talking over the actual tailnet. Bearcave was not
available to test against.

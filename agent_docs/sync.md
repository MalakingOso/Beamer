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

Everything under `sync/` is the same on every machine, given time and a
server. Everything directly under the config root is not, and Task 10 does
not touch any of it except to read `config.sync.url`.

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

**Attachments do not sync.** `notes.automerge` carries a note's *reference*
to an attachment, the content hash and extension `Attachment` stores, but
never the bytes themselves. `automerge::sync` moves changes to the document;
it was never given a channel for the separate `attachments/<hash>.<ext>`
files those changes point at. Concretely: dictate a note with a dropped image
on machine A, and once it syncs, machine B sees the note, sees that it has an
attachment, and has no bytes for it. `sticky_blocks.rs` renders whatever its
missing-file card looks like, because `attachments_dir(&config_dir)` on B
simply does not have `<hash>.<ext>` on disk.

This is the top open question this task leaves behind, not a rounding error:
a note that looks complete on one machine can look broken on the other, with
nothing in this protocol able to explain why or fix it. `todo.md`'s sync
section has the fuller shape of what is missing, including that the
cross-machine deletion race Task 8 flagged now has a different trigger
(sync-delivered changes) but the same unresolved shape.

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
exactly what `mark_pending_save`/`take_pending_save` exist to catch. See the
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

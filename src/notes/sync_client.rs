//! Live sync client: keeps the local automerge document converged with
//! `sync_server` over a WebSocket, using `automerge::sync`.
//!
//! **Offline-first.** A note is a working document with or without a server.
//! `run_client` reconnects forever with backoff and never surfaces a dropped
//! connection as anything worse than a debug log line: losing the server
//! loses live propagation and nothing else, because the document on disk
//! stays the source of truth on each machine either way.
//!
//! **Threading.** The raw socket lives on a plain `tokio::spawn` task
//! (`socket_task`) that only ever moves bytes between the WebSocket and two
//! channels; it holds no `Signal`. Applying an incoming change writes
//! `notes`/`tasks`, which *are* `Signal`s backed by a thread-local
//! generational-box arena, so that step (`apply_incoming`) runs inside the
//! Dioxus coroutine started by `use_sync_client`, never inside the socket
//! task. See `agent_docs/dioxus_architecture.md`.
//!
//! **Who writes the file.** Nobody here. `apply_incoming` mutates the
//! in-memory document and calls `SyncDoc::mark_pending_save`; the existing
//! 500ms tick in `flush.rs` is still the only place that calls `SyncDoc::save`.
//!
//! **Reconcile before merge, same as `flush.rs`.** An edit sitting in
//! `notes.notes`/`tasks.tasks` but not yet reconciled into the document (the
//! 500ms tick has not run since the keystroke) is not yet visible to
//! `receive_sync_message`. Hydrating straight from the document after
//! applying an incoming message, without reconciling our own pending edits
//! in first, would silently throw that edit away: the hydrate replaces
//! `notes.notes` wholesale, so the next tick's reconcile sees no diff and
//! there is nothing left to recover. `flush.rs`'s own module doc names this
//! exact hazard as the reason it reconciles before merging; the document-only
//! core here (`reconcile_receive_and_hydrate`) follows the same order for the
//! same reason.

use std::time::Duration;

use automerge::sync::{Message as SyncMessage, State as SyncState, SyncDoc as _};
use dioxus::prelude::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::config::Config;

use super::sync_doc::SyncHandle;
use super::task_store::TaskStore;
use super::{doc_notes, doc_tasks, NoteStore};

/// First retry delay, and the ceiling it backs off to. A note-taking app
/// reconnecting to a desktop on a tailnet has no reason to hammer faster than
/// once a second, and no reason to ever wait longer than thirty.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Whether a sync URL is configured at all. Pulled out as its own function so
/// "an empty URL means the client never starts" is a plain assertion against
/// a pure function, not something that needs a Dioxus render tree to observe.
pub fn should_start(url: &str) -> bool {
    !url.trim().is_empty()
}

/// Start the live-sync client. Call once, from `App()`.
///
/// A no-op when `config.sync.url` is empty: sync is off until a machine is
/// told a server exists, matching `note_hotkey`'s empty-means-off precedent.
/// The URL is read once, at first render, the same way the dictation
/// hotkey's initial binding is read in `app.rs`; a config edit takes effect
/// on the next restart, not live. Settings has no toggle for this yet either;
/// wiring one is later work, not part of this task.
pub fn use_sync_client(config: Signal<Config>, doc: SyncHandle, notes: Signal<NoteStore>, tasks: Signal<TaskStore>) {
    use_hook(move || {
        let url = config.peek().sync.url.trim().to_string();
        if !should_start(&url) {
            return;
        }
        spawn(run_client(url, doc, notes, tasks));
    });
}

/// Reconnect forever. Returns only if the coroutine's owning scope drops.
///
/// Backoff resets the moment a connection is *established*, regardless of how
/// it later ends: a connection that ran for hours and then dropped with a
/// read or write error deserves the same fast retry as one that closed
/// politely. Waiting up to 30s to retry after an hours-long, error-terminated
/// connection would be the wrong lesson to draw from that history.
async fn run_client(url: String, doc: SyncHandle, mut notes: Signal<NoteStore>, mut tasks: Signal<TaskStore>) {
    let mut backoff = BACKOFF_START;
    loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => {
                backoff = BACKOFF_START;
                match run_connection(ws, &doc, &mut notes, &mut tasks).await {
                    Ok(()) => tracing::info!("sync connection to {url} closed cleanly"),
                    Err(e) => tracing::debug!("sync connection to {url} dropped: {e}"),
                }
            }
            Err(e) => {
                tracing::debug!("could not connect to {url}: {e}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One connection's worth of the sync protocol, given an already-established
/// socket. Returns when the socket closes or errors; the caller decides what
/// happens next.
async fn run_connection(
    ws: WsStream,
    doc: &SyncHandle,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
) -> anyhow::Result<()> {
    let (ws_write, ws_read) = ws.split();

    // Decoded messages in, raw bytes out. The socket task below is the only
    // thing that touches the WebSocket; everything past the channels runs on
    // the Dioxus coroutine that owns `notes`/`tasks`.
    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<SyncMessage>();
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    let socket = tokio::spawn(socket_task(ws_write, ws_read, in_tx, out_rx));

    let mut state = SyncState::new();
    send_pending(doc, &mut state, &out_tx);

    // A local edit that lands while this connection is already open and
    // idle still has to reach the peer. `in_rx.recv()` alone only reacts to
    // messages arriving *from* the peer, so on its own an idle-but-connected
    // client would only ever push what it had at connect time, and every
    // dictation made after that would wait for a reconnect that (by design,
    // see the module doc) may not come for a long time. This interval is
    // what notices our own document moving instead.
    //
    // This is not the "never poll the server" rule from
    // `agent_docs/local_inference.md`: that rule is about polling
    // `llama-server`'s HTTP status and pinning a model in VRAM forever. This
    // is a read-only `generate_sync_message` against our own in-memory
    // document: cheap, and a no-op send whenever there is nothing new, since
    // the protocol itself returns `None`.
    let mut local_check = tokio::time::interval(Duration::from_millis(500));
    local_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            incoming = in_rx.recv() => {
                let Some(msg) = incoming else { break };
                apply_incoming(doc, notes, tasks, &mut state, msg);
                send_pending(doc, &mut state, &out_tx);
            }
            _ = local_check.tick() => {
                send_pending(doc, &mut state, &out_tx);
            }
        }
    }

    // `in_rx` only closes once `in_tx` is dropped, which happens when
    // `socket_task` returns, so by the time we get here the task is already
    // finishing or finished. Awaiting it (rather than the `abort` this used
    // to be) is what lets a write or read error on the socket propagate up
    // as `Err` instead of being reported as a clean close.
    match socket.await {
        Ok(result) => result,
        Err(join_err) => anyhow::bail!("sync socket task panicked: {join_err}"),
    }
}

/// The dumb pipe. Holds no `Signal`, so a plain `tokio::spawn` (not Dioxus's
/// `spawn`) is safe even though desktop's tokio runtime is multi-threaded.
///
/// Returns `Ok(())` only for a clean end: the peer closed, the stream ended,
/// or our own side stopped listening first (the channels closed on us, which
/// is `run_connection` having already decided to stop for its own reason). A
/// decode failure is logged and skipped, not fatal: a malformed message from
/// an otherwise-healthy peer is not the same failure as a dead socket. A read
/// or write error on the socket itself is `Err`, so `run_connection` (and
/// then `run_client`'s log line) can tell "the peer went away cleanly" apart
/// from "the connection broke".
async fn socket_task(
    mut ws_write: futures_util::stream::SplitSink<WsStream, WsMessage>,
    mut ws_read: futures_util::stream::SplitStream<WsStream>,
    in_tx: tokio::sync::mpsc::UnboundedSender<SyncMessage>,
    mut out_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            incoming = ws_read.next() => {
                match incoming {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        match SyncMessage::decode(&bytes) {
                            Ok(msg) => {
                                if in_tx.send(msg).is_err() {
                                    return Ok(());
                                }
                            }
                            Err(e) => tracing::warn!("could not decode an incoming sync message: {e}"),
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {} // ping/pong/text: nothing on this protocol sends them
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            outgoing = out_rx.recv() => {
                match outgoing {
                    Some(bytes) => {
                        if let Err(e) = ws_write.send(WsMessage::Binary(bytes.into())).await {
                            return Err(e.into());
                        }
                    }
                    None => return Ok(()),
                }
            }
        }
    }
}

/// Generate the next outgoing message, if the protocol has one to send, and
/// hand it to the socket task. A pure read against the shared document: never
/// mutates it, so this is safe to call from anywhere, including right after
/// `apply_incoming` has just written to it.
fn send_pending(doc: &SyncHandle, state: &mut SyncState, out_tx: &tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
    let mut guard = doc.lock();
    let msg = guard.doc_mut().sync().generate_sync_message(state);
    drop(guard);
    if let Some(msg) = msg {
        let _ = out_tx.send(msg.encode());
    }
}

/// The document-only half of applying one incoming sync message: reconcile
/// our own pending edits in first, then merge, then say what (if anything)
/// needs hydrating back into the stores.
///
/// No `Signal` anywhere in this function, which is deliberate: it is what
/// lets `sync_tests.rs` drive it directly, against the same `Machine` harness
/// the file-based merge tests already use, with no Dioxus runtime in sight.
///
/// Mirrors `flush::run_document_pass`'s own order (reconcile, then merge,
/// then hydrate only if heads moved) for the reason given in the module doc:
/// skipping the reconcile step here is exactly the bug that doc comment on
/// `flush.rs` warns against, just reached by a second path instead of the
/// first.
pub(crate) fn reconcile_receive_and_hydrate(
    handle: &SyncHandle,
    notes: &NoteStore,
    tasks: &TaskStore,
    state: &mut SyncState,
    msg: SyncMessage,
) -> Option<(doc_notes::Hydrated, doc_tasks::Hydrated)> {
    let mut guard = handle.lock();
    if guard.is_read_only() {
        tracing::warn!("sync document is read-only; dropping an incoming change");
        return None;
    }

    let before = guard.heads();

    if let Err(e) = doc_notes::reconcile(&mut guard, &notes.notes, &notes.unreadable_notes) {
        tracing::error!("Could not write notes into the sync document before merging: {e}");
    }
    if let Err(e) = doc_tasks::reconcile(&mut guard, &tasks.tasks, &tasks.unreadable_tasks) {
        tracing::error!("Could not write tasks into the sync document before merging: {e}");
    }

    if let Err(e) = guard.doc_mut().sync().receive_sync_message(state, msg) {
        tracing::warn!("could not apply an incoming sync message: {e}");
        return None;
    }

    // Most messages in this protocol carry no changes at all: an initial
    // handshake, an ack, a peer telling us it has nothing new. Hydrating and
    // dirtying both stores on every one of those would rewrite `notes.json`
    // and wake every signal subscriber for no reason, on every message a
    // live connection exchanges. This also covers the case where only our
    // own reconcile above moved anything: nothing arrived worth hydrating
    // for, since `notes`/`tasks` already hold that content.
    if guard.heads() == before {
        return None;
    }
    guard.mark_pending_save();
    Some((doc_notes::hydrate(&guard), doc_tasks::hydrate(&guard)))
}

/// The `Signal`-writing half: peek the stores for `reconcile_receive_and_hydrate`,
/// then, if it found something worth hydrating, write the result back.
fn apply_incoming(
    doc: &SyncHandle,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    state: &mut SyncState,
    msg: SyncMessage,
) {
    let outcome = {
        let notes_ref = notes.peek();
        let tasks_ref = tasks.peek();
        reconcile_receive_and_hydrate(doc, &notes_ref, &tasks_ref, state, msg)
    };
    let Some((n, t)) = outcome else {
        return;
    };

    let mut notes_mut = notes.write();
    notes_mut.notes = n.notes;
    notes_mut.unreadable_notes = n.unreadable;
    notes_mut.dirty = true;
    notes_mut.doc_dirty = true;
    drop(notes_mut);

    let mut tasks_mut = tasks.write();
    tasks_mut.tasks = t.tasks;
    tasks_mut.unreadable_tasks = t.unreadable;
    tasks_mut.dirty = true;
    tasks_mut.doc_dirty = true;
}

#[cfg(test)]
#[path = "sync_client/tests.rs"]
mod tests;

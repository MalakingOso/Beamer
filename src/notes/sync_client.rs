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

use std::time::Duration;

use automerge::sync::{Message as SyncMessage, State as SyncState, SyncDoc as _};
use dioxus::prelude::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::config::Config;

use super::sync_doc::SyncHandle;
use super::task_store::TaskStore;
use super::{doc_notes, doc_tasks, NoteStore};

/// First retry delay, and the ceiling it backs off to. A note-taking app
/// reconnecting to a desktop on a tailnet has no reason to hammer faster than
/// once a second, and no reason to ever wait longer than thirty.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

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
async fn run_client(url: String, doc: SyncHandle, mut notes: Signal<NoteStore>, mut tasks: Signal<TaskStore>) {
    let mut backoff = BACKOFF_START;
    loop {
        match connect_and_sync(&url, &doc, &mut notes, &mut tasks).await {
            Ok(()) => {
                tracing::info!("sync connection to {url} closed cleanly");
                backoff = BACKOFF_START;
            }
            Err(e) => {
                tracing::debug!("sync connection to {url} dropped: {e}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One connection's worth of the sync protocol. Returns when the socket
/// closes or errors; the caller decides what happens next.
async fn connect_and_sync(
    url: &str,
    doc: &SyncHandle,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(url).await?;
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
    // finishing or finished. `abort` is a formality, not a race.
    socket.abort();
    Ok(())
}

/// The dumb pipe. Holds no `Signal`, so a plain `tokio::spawn` (not Dioxus's
/// `spawn`) is safe even though desktop's tokio runtime is multi-threaded.
async fn socket_task(
    mut ws_write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        WsMessage,
    >,
    mut ws_read: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    >,
    in_tx: tokio::sync::mpsc::UnboundedSender<SyncMessage>,
    mut out_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) {
    loop {
        tokio::select! {
            incoming = ws_read.next() => {
                match incoming {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        match SyncMessage::decode(&bytes) {
                            Ok(msg) => {
                                if in_tx.send(msg).is_err() {
                                    return;
                                }
                            }
                            Err(e) => tracing::warn!("could not decode an incoming sync message: {e}"),
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => return,
                    Some(Ok(_)) => {} // ping/pong/text: nothing on this protocol sends them
                    Some(Err(e)) => {
                        tracing::debug!("sync socket error: {e}");
                        return;
                    }
                }
            }
            outgoing = out_rx.recv() => {
                match outgoing {
                    Some(bytes) => {
                        if ws_write.send(WsMessage::Binary(bytes.into())).await.is_err() {
                            return;
                        }
                    }
                    None => return,
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

/// Apply one incoming sync message to the shared document, then hydrate
/// `notes`/`tasks` from the result so the UI reflects it immediately.
///
/// Mirrors `flush::run_document_pass`'s own merged branch (same hydrate
/// calls, same two fields written back) because this is the same situation
/// by a different route: content arrived from another machine and both
/// stores need to catch up. The difference is `mark_pending_save`; see its
/// doc comment on `SyncDoc` for why a heads comparison alone would miss this.
fn apply_incoming(
    doc: &SyncHandle,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    state: &mut SyncState,
    msg: SyncMessage,
) {
    let hydrated = {
        let mut guard = doc.lock();
        if guard.is_read_only() {
            tracing::warn!("sync document is read-only; dropping an incoming change");
            return;
        }
        let before = guard.heads();
        if let Err(e) = guard.doc_mut().sync().receive_sync_message(state, msg) {
            tracing::warn!("could not apply an incoming sync message: {e}");
            return;
        }
        // Most messages in this protocol carry no changes at all: an initial
        // handshake, an ack, a peer telling us it has nothing new. Hydrating
        // and dirtying both stores on every one of those would rewrite
        // `notes.json` and wake every signal subscriber for no reason, on
        // every message a live connection exchanges.
        if guard.heads() == before {
            return;
        }
        guard.mark_pending_save();
        (doc_notes::hydrate(&guard), doc_tasks::hydrate(&guard))
    };
    let (n, t) = hydrated;

    let mut notes = notes.write();
    notes.notes = n.notes;
    notes.unreadable_notes = n.unreadable;
    notes.dirty = true;
    notes.doc_dirty = true;
    drop(notes);

    let mut tasks = tasks.write();
    tasks.tasks = t.tasks;
    tasks.unreadable_tasks = t.unreadable;
    tasks.dirty = true;
    tasks.doc_dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_url_never_starts() {
        assert!(!should_start(""));
        assert!(!should_start("   "));
    }

    #[test]
    fn a_configured_url_starts() {
        assert!(should_start("wss://callisto.taila63f23.ts.net/sync"));
    }

    /// The deterministic part of the protocol: two in-process documents,
    /// synced purely through `automerge::sync::State` and
    /// `Message::encode`/`decode`, with no socket anywhere. This is the
    /// scenario `connect_and_sync`/`socket_task` exist to carry over a
    /// WebSocket, so proving it converges here is what actually tests the
    /// protocol; wiring it through a real connection would only test tokio.
    #[test]
    fn two_documents_converge_over_encoded_messages() {
        use automerge::transaction::Transactable;
        use automerge::{AutoCommit, ReadDoc, ROOT};

        let mut a = AutoCommit::new();
        a.put(ROOT, "from_a", "hello").unwrap();
        a.commit();

        let mut b = AutoCommit::new();
        b.put(ROOT, "from_b", "world").unwrap();
        b.commit();

        let mut a_state = SyncState::new();
        let mut b_state = SyncState::new();

        // Drive both directions until neither has anything left to send,
        // exactly as the crate's own sync module doc example does.
        loop {
            let a_to_b = a.sync().generate_sync_message(&mut a_state);
            if let Some(msg) = a_to_b.clone() {
                let wire = msg.encode();
                let decoded = SyncMessage::decode(&wire).unwrap();
                b.sync().receive_sync_message(&mut b_state, decoded).unwrap();
            }
            let b_to_a = b.sync().generate_sync_message(&mut b_state);
            if let Some(msg) = b_to_a.clone() {
                let wire = msg.encode();
                let decoded = SyncMessage::decode(&wire).unwrap();
                a.sync().receive_sync_message(&mut a_state, decoded).unwrap();
            }
            if a_to_b.is_none() && b_to_a.is_none() {
                break;
            }
        }

        assert_eq!(a.get(ROOT, "from_b").unwrap().unwrap().0.to_str(), Some("world"));
        assert_eq!(b.get(ROOT, "from_a").unwrap().unwrap().0.to_str(), Some("hello"));
        assert_eq!(a.get_heads(), b.get_heads());
    }

    /// A connection drops mid-exchange: the peer's `State` is thrown away,
    /// as a real reconnect does, since nothing persists per-peer sync state
    /// across a socket close. Resuming with a fresh `State` still converges;
    /// it just costs a fuller first message, the price offline-first sync
    /// pays for storing no session state on either side.
    #[test]
    fn a_dropped_connection_reconverges_with_a_fresh_state() {
        use automerge::transaction::Transactable;
        use automerge::{AutoCommit, ReadDoc, ROOT};

        let mut a = AutoCommit::new();
        a.put(ROOT, "note", "first draft").unwrap();
        a.commit();

        let mut b = AutoCommit::new();

        let mut a_state = SyncState::new();
        let mut b_state = SyncState::new();

        // One exchange, then the connection drops before convergence: only
        // a's first message ever reaches b.
        let first = a.sync().generate_sync_message(&mut a_state).expect("a has something to send");
        b.sync()
            .receive_sync_message(&mut b_state, SyncMessage::decode(&first.encode()).unwrap())
            .unwrap();
        assert_ne!(a.get_heads(), b.get_heads(), "the drop must land before convergence, or this proves nothing");

        // Reconnect: both sides start over with a fresh sync state, as
        // `connect_and_sync` does on every call.
        let mut a_state = SyncState::new();
        let mut b_state = SyncState::new();
        loop {
            let a_to_b = a.sync().generate_sync_message(&mut a_state);
            if let Some(msg) = a_to_b.clone() {
                b.sync()
                    .receive_sync_message(&mut b_state, SyncMessage::decode(&msg.encode()).unwrap())
                    .unwrap();
            }
            let b_to_a = b.sync().generate_sync_message(&mut b_state);
            if let Some(msg) = b_to_a.clone() {
                a.sync()
                    .receive_sync_message(&mut a_state, SyncMessage::decode(&msg.encode()).unwrap())
                    .unwrap();
            }
            if a_to_b.is_none() && b_to_a.is_none() {
                break;
            }
        }

        assert_eq!(a.get_heads(), b.get_heads(), "a fresh state must still re-converge after a drop");
        assert_eq!(b.get(ROOT, "note").unwrap().unwrap().0.to_str(), Some("first draft"));
    }

    /// `encode` then `decode` round-trips a message byte for byte in the
    /// fields that matter: what the wire actually carries.
    #[test]
    fn message_framing_round_trips() {
        use automerge::transaction::Transactable;
        use automerge::{AutoCommit, ROOT};

        let mut doc = AutoCommit::new();
        doc.put(ROOT, "key", "value").unwrap();
        doc.commit();

        let mut state = SyncState::new();
        let msg = doc.sync().generate_sync_message(&mut state).expect("a fresh document has something to send");

        let wire = msg.clone().encode();
        let decoded = SyncMessage::decode(&wire).unwrap();

        assert_eq!(decoded, msg);
    }
}

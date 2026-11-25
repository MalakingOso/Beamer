//! Live sync client: converges the local automerge document with
//! `bin/sync_server.rs` over a WebSocket. Started once from `App()` via
//! `use_sync_client`; off when `config.sync.url` is empty. See `agent_docs/sync.md`.
//!
//! Offline-first: a dropped connection only loses live propagation, never data.
//! The socket task holds no `Signal`; incoming changes are applied on the Dioxus
//! coroutine. Nothing here writes the file: the 500 ms tick (`flush`) still owns
//! `SyncDoc::save`, so a merge here calls `mark_pending_save`. Like `flush.rs`,
//! reconcile precedes merge, or the wholesale hydrate throws away a pending local edit.

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

/// Reconnect backoff: 1s start, 30s ceiling.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Minimum healthy connection duration before resetting backoff (prevents 1s
/// reconnect storms when a peer drops immediately after WebSocket upgrade).
const HEALTHY_CONNECTION: Duration = Duration::from_secs(10);
/// Handshake, heartbeat, idle and write timeouts so half-open TCP connections
/// across sleep/wake or Tailscale drops never hang `SyncStatus::Connected`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Whether a sync URL is configured at all. Empty means the client never starts.
pub fn should_start(url: &str) -> bool {
    !url.trim().is_empty()
}

/// Live state of the sync connection for the Settings page. Written only from
/// `run_client`'s coroutine, on socket events — never polled.
#[derive(Clone, PartialEq, Default)]
pub enum SyncStatus {
    /// No URL configured, or the URL changed since the last restart.
    #[default]
    Off,
    Connecting,
    Connected,
    /// Failed attempt or dropped connection; `detail` is the raw socket error.
    Disconnected { detail: String },
}

/// What the Settings page needs: live state, plus the URL actually started
/// with (fixed at mount, so an unsaved edit reads as unsaved, not connected).
#[derive(Clone, PartialEq)]
pub struct SyncClientHandle {
    pub status: Signal<SyncStatus>,
    pub started_url: String,
}

/// Start the live-sync client. Call once, from `App()`. A no-op on an empty
/// URL. The URL is read once at first render; edits take effect on restart.
pub fn use_sync_client(config: Signal<Config>, doc: SyncHandle, notes: Signal<NoteStore>, tasks: Signal<TaskStore>) -> SyncClientHandle {
    let status = use_signal(SyncStatus::default);
    let started_url = use_hook(move || {
        let url = config.peek().sync.url.trim().to_string();
        if should_start(&url) {
            spawn(run_client(url.clone(), doc, notes, tasks, status));
        }
        url
    });
    SyncClientHandle { status, started_url }
}

/// Reconnect forever. Backoff resets only once a connection has stayed healthy
/// for `HEALTHY_CONNECTION`, preventing 1s storms on immediate post-handshake drops.
async fn run_client(
    url: String,
    doc: SyncHandle,
    mut notes: Signal<NoteStore>,
    mut tasks: Signal<TaskStore>,
    mut status: Signal<SyncStatus>,
) {
    let mut backoff = BACKOFF_START;
    loop {
        status.set(SyncStatus::Connecting);
        let connect_result =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&url)).await;
        match connect_result {
            Ok(Ok((ws, _))) => {
                let connected_at = tokio::time::Instant::now();
                status.set(SyncStatus::Connected);
                match run_connection(ws, &doc, &mut notes, &mut tasks).await {
                    Ok(()) => {
                        tracing::info!("sync connection to {url} closed cleanly");
                        status.set(SyncStatus::Disconnected { detail: "the connection closed".to_string() });
                    }
                    Err(e) => {
                        tracing::warn!("sync connection to {url} dropped: {e}");
                        status.set(SyncStatus::Disconnected { detail: e.to_string() });
                    }
                }
                if connected_at.elapsed() >= HEALTHY_CONNECTION {
                    backoff = BACKOFF_START;
                }
            }
            Ok(Err(e)) => {
                tracing::info!("could not connect to {url}: {e}");
                status.set(SyncStatus::Disconnected { detail: e.to_string() });
            }
            Err(_) => {
                let msg = format!("connection timed out after {}s", CONNECT_TIMEOUT.as_secs());
                tracing::info!("could not connect to {url}: {msg}");
                status.set(SyncStatus::Disconnected { detail: msg });
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One connection's worth of the sync protocol. Returns when the socket
/// closes or errors; the caller decides what happens next.
async fn run_connection(
    ws: WsStream,
    doc: &SyncHandle,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
) -> anyhow::Result<()> {
    let (ws_write, ws_read) = ws.split();

    // Decoded messages in, raw bytes out. Only the socket task touches the
    // WebSocket; past the channels everything runs on the Dioxus coroutine.
    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<SyncMessage>();
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    let socket = tokio::spawn(socket_task(ws_write, ws_read, in_tx, out_rx));

    let mut state = SyncState::new();
    send_pending(doc, &mut state, &out_tx);

    // An idle-but-connected client must still push local edits made after
    // connect. A read-only `generate_sync_message` against our own in-memory
    // document; a no-op send when nothing is new.
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

    // Await (don't abort) so a socket read/write error propagates as `Err`,
    // not a clean close.
    match socket.await {
        Ok(result) => result,
        Err(join_err) => anyhow::bail!("sync socket task panicked: {join_err}"),
    }
}

/// The dumb pipe. Holds no `Signal`, so plain `tokio::spawn` is safe.
/// Sends periodic WebSocket Pings and enforces idle + write timeouts so
/// half-open connections (sleep/wake, Tailscale drops) disconnect and reconnect.
async fn socket_task(
    mut ws_write: futures_util::stream::SplitSink<WsStream, WsMessage>,
    mut ws_read: futures_util::stream::SplitStream<WsStream>,
    in_tx: tokio::sync::mpsc::UnboundedSender<SyncMessage>,
    mut out_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) -> anyhow::Result<()> {
    let mut last_activity = tokio::time::Instant::now();
    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // First tick fires immediately; advance past it.
    ping_tick.tick().await;

    loop {
        tokio::select! {
            incoming = ws_read.next() => {
                last_activity = tokio::time::Instant::now();
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
                    Some(Ok(_)) => {} // ping/pong/text: activity timestamp updated above
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            outgoing = out_rx.recv() => {
                match outgoing {
                    Some(bytes) => {
                        match tokio::time::timeout(
                            WRITE_TIMEOUT,
                            ws_write.send(WsMessage::Binary(bytes.into())),
                        )
                        .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => return Err(e.into()),
                            Err(_) => anyhow::bail!(
                                "sync write timed out after {}s",
                                WRITE_TIMEOUT.as_secs()
                            ),
                        }
                    }
                    None => return Ok(()),
                }
            }
            _ = ping_tick.tick() => {
                if last_activity.elapsed() > IDLE_TIMEOUT {
                    anyhow::bail!(
                        "sync server silent for over {}s; reconnecting",
                        IDLE_TIMEOUT.as_secs()
                    );
                }
                match tokio::time::timeout(
                    WRITE_TIMEOUT,
                    ws_write.send(WsMessage::Ping(Vec::new().into())),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => anyhow::bail!(
                        "sync ping timed out after {}s",
                        WRITE_TIMEOUT.as_secs()
                    ),
                }
            }
        }
    }
}

/// Generate the next outgoing message, if any, and hand it to the socket task.
/// A pure read against the shared document; never mutates it.
fn send_pending(doc: &SyncHandle, state: &mut SyncState, out_tx: &tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
    let mut guard = doc.lock();
    let msg = guard.doc_mut().sync().generate_sync_message(state);
    drop(guard);
    if let Some(msg) = msg {
        let _ = out_tx.send(msg.encode());
    }
}

/// The document-only half of applying one incoming message: reconcile ours
/// first, then merge, then hydrate if heads moved. `Signal`-free so tests can
/// drive it without a Dioxus runtime.
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

    let mut reconcile_ok = true;
    if let Err(e) = doc_notes::reconcile(&mut guard, &notes.notes, &notes.unreadable_notes) {
        tracing::error!("Could not write notes into the sync document before merging: {e}");
        reconcile_ok = false;
    }
    if let Err(e) = doc_tasks::reconcile(&mut guard, &tasks.tasks, &tasks.unreadable_tasks) {
        tracing::error!("Could not write tasks into the sync document before merging: {e}");
        reconcile_ok = false;
    }

    // Capture heads *after* local reconcile so a remote ACK carrying zero new
    // changes during local typing doesn't trigger a full hydrate/re-render.
    let before = guard.heads();

    if let Err(e) = guard.doc_mut().sync().receive_sync_message(state, msg) {
        tracing::warn!("could not apply an incoming sync message: {e}");
        return None;
    }

    // Most messages carry no changes; hydrating on each would rewrite the
    // mirrors and wake every subscriber for nothing.
    if guard.heads() == before {
        return None;
    }
    guard.mark_pending_save();
    if !reconcile_ok {
        // Skip hydrating over local edits that failed to reconcile.
        return None;
    }
    Some((doc_notes::hydrate(&guard), doc_tasks::hydrate(&guard)))
}

/// The `Signal`-writing half: hydrate via `reconcile_receive_and_hydrate`, then write back.
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

    // Peek *before* taking `Signal::write()`: in Dioxus 0.7, dropping a write
    // guard notifies every subscriber even when nothing was mutated.
    let notes_changed = {
        let cur = notes.peek();
        cur.notes != n.notes || cur.unreadable_notes != n.unreadable
    };
    if notes_changed {
        let mut notes_mut = notes.write();
        notes_mut.notes = n.notes;
        notes_mut.unreadable_notes = n.unreadable;
        notes_mut.dirty = true;
        notes_mut.doc_dirty = true;
    }

    let tasks_changed = {
        let cur = tasks.peek();
        cur.tasks != t.tasks || cur.unreadable_tasks != t.unreadable
    };
    if tasks_changed {
        let mut tasks_mut = tasks.write();
        tasks_mut.tasks = t.tasks;
        tasks_mut.unreadable_tasks = t.unreadable;
        tasks_mut.dirty = true;
        tasks_mut.doc_dirty = true;
    }
}

#[cfg(test)]
#[path = "sync_client/tests.rs"]
mod tests;

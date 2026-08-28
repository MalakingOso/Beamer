//! `sync_server`: the authoritative replica of `notes.automerge`, reachable
//! only over the tailnet via `tailscale serve`'s `/sync` proxy.
//!
//! Runs on `callisto` beside `llama-server`. Owns one automerge document,
//! shared across every connected peer behind a `Mutex`, and keeps one
//! `automerge::sync::State` per connection, the standard `automerge::sync`
//! relay shape from the crate's own doc example, just with a WebSocket
//! instead of the loop in that example driving both sides directly.
//!
//! ⚠️ **Binds loopback only, and refuses to start otherwise.** There is no
//! auth anywhere in this protocol; `tailscaled` is what authenticates a
//! caller, the same trust boundary `llama-beamer.service` already relies on,
//! and it only vouches for traffic that already reached loopback through the
//! `tailscale serve` proxy. See `agent_docs/sync.md`.
//!
//! ## Why this file looks the way it does
//!
//! There is no `src/lib.rs`, so a second binary cannot `use beamer::…`.
//! `src/notes/sync_doc.rs` was written free of crate-rooted paths for exactly
//! this reason (see its own module doc), so it `#[path]`-includes cleanly,
//! same trick `task_eval.rs` uses for `llm/mod.rs` and the three `notes/`
//! files it needs. This binary needs nothing else from `notes/`: it never
//! reads a note or a task, only relays whatever the document holds, so
//! `doc_notes`/`doc_tasks` (which do reach for `super::model`/`super::task`)
//! stay out of it entirely.
//!
//! ## Run it
//!
//! ```text
//! cargo run --bin sync_server -- --bind 127.0.0.1:8081
//! ```

// `sync_doc.rs` is `#[path]`-included whole, the same tradeoff `task_eval.rs`
// makes with `llm/mod.rs`: the alternative is scattering `#[allow]` through
// shared source to suit one consumer that only needs a slice of it. The tree
// stays at zero warnings; this file, deliberately, does not need to.
#![allow(dead_code)]

#[path = "../notes/sync_doc.rs"]
mod sync_doc;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use automerge::sync::{Message as SyncMessage, State as SyncState, SyncDoc as _};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::WebSocketStream;

use sync_doc::{SyncDoc, SyncHandle};

const DEFAULT_BIND: &str = "127.0.0.1:8081";

/// `<config_dir>/sync/notes.automerge`, matching `notes::sync_dir` and
/// `notes::notes_document_path`. Reimplemented rather than imported: those
/// live in `notes/mod.rs`, which reaches for `crate::config::Config` to
/// resolve its own default paths, and dragging that whole module tree in for
/// two lines of path-joining would be a worse trade than the duplication.
fn notes_document_path(config_dir: &Path) -> PathBuf {
    config_dir.join("sync").join("notes.automerge")
}

/// Deliberately **not** `Config::config_dir()`'s `Beamer` leaf. This box
/// (`callisto`) can also run a Beamer install directly, for testing this
/// feature if nothing else, and defaulting to the same directory the app
/// itself uses would make every operator's first `cargo run --bin
/// sync_server` a silent, undocumented file-sharing arrangement between two
/// independent processes.
///
/// Two processes racing a write to the same `notes.automerge` is safe now
/// (`SyncDoc::save`'s temp name is pid-scoped, so a rename can no longer
/// collide with another process's temp file), but two processes still race
/// the final `rename` onto one path, and an operator who wants that sharing
/// on purpose should ask for it explicitly with `--config-dir`, not get it by
/// omission. See `agent_docs/sync.md`.
fn default_config_dir() -> PathBuf {
    let base = dirs::config_dir().expect("Could not determine config directory");
    base.join("BeamerSyncServer")
}

/// Refuse anything but loopback. `tailscaled` is this socket's entire
/// authentication story (see the module doc) and it only covers loopback,
/// so binding a routable address would serve the whole corpus, unauthenticated,
/// to whatever can reach that address.
fn ensure_loopback(addr: SocketAddr) -> Result<()> {
    if !addr.ip().is_loopback() {
        anyhow::bail!(
            "refusing to bind {addr}: not a loopback address. This server has no \
             protocol-level auth of its own; tailscaled, via `tailscale serve`, is what \
             authenticates every caller, and it only does that for connections that already \
             reached loopback. Binding anything else exposes the whole corpus."
        );
    }
    Ok(())
}

struct Args {
    bind: SocketAddr,
    config_dir: PathBuf,
}

fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Args> {
    let mut bind: SocketAddr = DEFAULT_BIND.parse().expect("DEFAULT_BIND is a valid address");
    let mut config_dir = default_config_dir();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                let v = args.next().context("--bind needs a value")?;
                bind = v.parse().with_context(|| format!("could not parse --bind {v}"))?;
            }
            "--config-dir" => {
                let v = args.next().context("--config-dir needs a value")?;
                config_dir = PathBuf::from(v);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(Args { bind, config_dir })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("sync_server=info,info")
        .init();

    let args = parse_args(std::env::args().skip(1))?;
    ensure_loopback(args.bind)?;

    let doc_path = notes_document_path(&args.config_dir);
    let (doc, load_error) = SyncDoc::open(doc_path.clone());
    if let Some(e) = load_error {
        // Not fatal: `SyncDoc::open` already quarantined what it could not
        // read and started this replica from genesis, same as any client
        // would. Peers still converge; this machine just isn't the one that
        // remembers what came before.
        tracing::error!("{e}");
    }
    tracing::info!("serving {} from {}", doc_path.display(), args.bind);
    let handle = SyncHandle::new(doc);

    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("could not bind {}", args.bind))?;

    // Bumped whenever any peer's changes land in the document, so every other
    // connected peer's task wakes and offers a fresh sync message. Without
    // this, a peer that had already caught up would only learn about a
    // second peer's edits the next time it happened to send something of its
    // own, which offline-first clients may not do for a while.
    let (changed_tx, _) = watch::channel(());

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let handle = handle.clone();
        let changed_tx = changed_tx.clone();
        tokio::spawn(async move {
            match serve_peer(stream, handle, changed_tx).await {
                Ok(()) => tracing::info!("peer {peer_addr} closed cleanly"),
                Err(e) => tracing::warn!("peer {peer_addr} disconnected: {e}"),
            }
        });
    }
}

/// How often an idle connection gets a ping, and how long with no traffic at
/// all (not even a pong) before it is presumed half-open and closed. Without
/// this, a peer that drops off the network without a clean TCP close (a
/// laptop that loses power, a network that black-holes instead of resetting)
/// parks its `serve_peer` task, its `sync::State` and its `watch::Receiver`
/// until the OS's own TCP timeout, which can be a long time. A clean
/// disconnect (`WsMessage::Close`, or the read returning an error) is already
/// handled without waiting for either of these.
const PING_INTERVAL: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// One connection, one `sync::State`, for as long as the socket lives.
/// Dropping the connection loses only that state; a reconnect starts a fresh
/// one and the protocol re-converges, at the cost of a fuller first message.
async fn serve_peer(stream: TcpStream, handle: SyncHandle, changed_tx: watch::Sender<()>) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut ws_write, mut ws_read) = ws.split();
    let mut changed_rx = changed_tx.subscribe();
    let mut state = SyncState::new();
    let mut last_activity = tokio::time::Instant::now();
    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    send_pending(&handle, &mut state, &mut ws_write).await?;

    loop {
        tokio::select! {
            incoming = ws_read.next() => {
                last_activity = tokio::time::Instant::now();
                match incoming {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        let msg = SyncMessage::decode(&bytes)
                            .context("could not decode an incoming sync message")?;
                        // Gated on the document actually moving: most
                        // messages in this protocol are handshakes and acks
                        // that carry no changes, and rewriting the document
                        // to disk (and waking every other connected peer)
                        // for one of those would make every idle connection
                        // a source of needless disk I/O and wakeups.
                        if apply_and_save(&handle, &mut state, msg)? {
                            // Our own send below already reflects this change
                            // for this peer; the broadcast is for every
                            // *other* peer whose `changed_rx.changed()` is
                            // waiting on it.
                            let _ = changed_tx.send(());
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {} // ping/pong/text: only pings/pongs arrive here, and tungstenite answers pings on our behalf
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            changed = changed_rx.changed() => {
                changed.context("the change-notification channel closed unexpectedly")?;
            }
            _ = ping_tick.tick() => {
                if last_activity.elapsed() > IDLE_TIMEOUT {
                    anyhow::bail!(
                        "peer sent nothing (not even a pong) for over {IDLE_TIMEOUT:?}; \
                         presuming the connection half-open and closing it"
                    );
                }
                ws_write.send(WsMessage::Ping(Vec::new().into())).await?;
            }
        }
        send_pending(&handle, &mut state, &mut ws_write).await?;
    }
}

/// Applies one incoming message and, only if it actually moved the
/// document's heads, saves and returns `true`. See the call site's comment
/// on why a no-op message must not trigger either.
fn apply_and_save(handle: &SyncHandle, state: &mut SyncState, msg: SyncMessage) -> Result<bool> {
    let mut doc = handle.lock();
    if doc.is_read_only() {
        tracing::warn!("sync document is read-only; dropping an incoming change from a peer");
        return Ok(false);
    }
    let before = doc.heads();
    doc.doc_mut().sync().receive_sync_message(state, msg)?;
    if doc.heads() == before {
        return Ok(false);
    }
    doc.save().context("could not save the sync document")?;
    Ok(true)
}

type WsSink = futures_util::stream::SplitSink<WebSocketStream<TcpStream>, WsMessage>;

/// Locking and generating happen in a plain function, never inline in the
/// `async fn` below: a `std::sync::MutexGuard` is `!Send`, and holding one
/// across an `.await`, even one that only lexically follows an explicit
/// `drop`, inside the same async fn, poisons that fn's whole future as
/// `!Send`, which `tokio::spawn` then refuses. A synchronous function has no
/// such ambiguity: the guard is provably gone the moment it returns.
fn generate_pending(handle: &SyncHandle, state: &mut SyncState) -> Option<SyncMessage> {
    let mut doc = handle.lock();
    let msg = doc.doc_mut().sync().generate_sync_message(state);
    msg
}

async fn send_pending(handle: &SyncHandle, state: &mut SyncState, ws_write: &mut WsSink) -> Result<()> {
    if let Some(msg) = generate_pending(handle, state) {
        ws_write.send(WsMessage::Binary(msg.encode().into())).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_v4_and_v6_are_accepted() {
        assert!(ensure_loopback("127.0.0.1:8081".parse().unwrap()).is_ok());
        assert!(ensure_loopback("[::1]:8081".parse().unwrap()).is_ok());
    }

    #[test]
    fn a_wildcard_or_routable_bind_is_refused() {
        assert!(ensure_loopback("0.0.0.0:8081".parse().unwrap()).is_err());
        // A real tailnet-range address (100.64.0.0/10), which is exactly the
        // mistake this check exists to catch: it looks private, and it is
        // reachable from every other device on the tailnet.
        assert!(ensure_loopback("100.101.102.103:8081".parse().unwrap()).is_err());
        assert!(ensure_loopback("[::]:8081".parse().unwrap()).is_err());
    }

    #[test]
    fn default_bind_parses_and_is_loopback() {
        let addr: SocketAddr = DEFAULT_BIND.parse().unwrap();
        assert!(ensure_loopback(addr).is_ok());
    }

    #[test]
    fn parses_bind_and_config_dir() {
        let args = parse_args(
            ["--bind", "127.0.0.1:9000", "--config-dir", "/tmp/beamer-test"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(args.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(args.config_dir, PathBuf::from("/tmp/beamer-test"));
    }

    #[test]
    fn unknown_argument_is_rejected() {
        assert!(parse_args(["--nope"].into_iter().map(String::from)).is_err());
    }

    /// The document format the server opens is exactly `SyncDoc`'s own, and
    /// that includes starting from the shared genesis change when nothing is
    /// on disk yet, the same call every Beamer client makes. This is a
    /// direct assertion on that fact, not a re-test of `SyncDoc` itself
    /// (which has its own suite in `notes::sync_tests`): if `sync_server`
    /// ever stopped calling `SyncDoc::open`/`new_document` and built a
    /// document some other way, this is what would catch it re-introducing
    /// the whole-map conflict genesis exists to prevent.
    #[test]
    fn a_fresh_server_document_starts_from_the_same_genesis_a_client_would() {
        let dir = std::env::temp_dir().join(format!("beamer_sync_server_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let (mut server_doc, load_error) = SyncDoc::open(notes_document_path(&dir));
        assert!(load_error.is_none(), "a fresh directory has nothing to fail to read");

        let mut client_doc = sync_doc::new_document();
        assert_eq!(
            server_doc.heads(),
            client_doc.get_heads(),
            "a server with no document on disk yet must start from the same genesis as a client, \
             or the first sync between them repeats the whole-map conflict genesis exists to prevent"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! The sync server: the authoritative replica of `notes.automerge` that every
//! machine's `notes::sync_client` converges with. Served on loopback only and
//! reached over the tailnet via `tailscale serve`'s `/sync` proxy. One shared
//! document plus one `automerge::sync::State` per connection.
//!
//! No protocol-level auth: `tailscaled` authenticates callers, and only for
//! traffic that already reached loopback. See `agent_docs/sync.md`.
//!
//! No `src/lib.rs`, so this binary `#[path]`-includes `notes/sync_doc/`
//! (written free of crate paths for this; its `fields` submodule resolves
//! beside it). It only relays the document, so `doc_notes`/`doc_tasks` stay
//! out.
//!
//! ```text
//! cargo run --bin sync_server -- --bind 127.0.0.1:8081
//! ```

// Whole-module include pulls in items this binary never calls.
#![allow(dead_code)]

#[path = "../notes/sync_doc/mod.rs"]
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

/// `<config_dir>/sync/notes.automerge`. Duplicated from `notes/mod.rs` to
/// avoid dragging its whole module tree in for two lines of path-joining.
fn notes_document_path(config_dir: &Path) -> PathBuf {
    config_dir.join("sync").join("notes.automerge")
}

/// `BeamerSyncServer`, deliberately not the app's own `Beamer` dir: a flagless
/// run on a machine that also runs Beamer would otherwise silently share its
/// `notes.automerge`. Opt into sharing explicitly with `--config-dir`.
fn default_config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .context("Could not determine config directory (pass --config-dir explicitly)")?;
    Ok(base.join("BeamerSyncServer"))
}

/// Refuse anything but loopback: `tailscaled` is the only auth, and it only
/// covers loopback. Binding a routable address would serve the corpus openly.
fn ensure_loopback(addr: SocketAddr) -> Result<()> {
    if !addr.ip().is_loopback() {
        anyhow::bail!(
            "refusing to bind {addr}: not a loopback address. This server has no \
             protocol-level auth; tailscaled only vouches for loopback traffic."
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
    let mut config_dir: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                let v = args.next().context("--bind needs a value")?;
                bind = v.parse().with_context(|| format!("could not parse --bind {v}"))?;
            }
            "--config-dir" => {
                let v = args.next().context("--config-dir needs a value")?;
                config_dir = Some(PathBuf::from(v));
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    let config_dir = match config_dir {
        Some(dir) => dir,
        None => default_config_dir()?,
    };
    Ok(Args { bind, config_dir })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("sync_server=info,info")),
        )
        .init();

    let args = parse_args(std::env::args().skip(1))?;
    ensure_loopback(args.bind)?;

    let doc_path = notes_document_path(&args.config_dir);
    let (doc, load_error) = SyncDoc::open(doc_path.clone());
    if let Some(e) = load_error {
        tracing::error!("{e}");
    }
    if doc.is_read_only() {
        anyhow::bail!(
            "refusing to start with a read-only document at {} — fix permissions/corruption first",
            doc_path.display()
        );
    }
    tracing::info!("serving {} from {}", doc_path.display(), args.bind);
    let handle = SyncHandle::new(doc);

    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("could not bind {}", args.bind))?;

    // Bumped on every landed change so caught-up peers wake and re-offer.
    let (changed_tx, _) = watch::channel(());

    loop {
        let (stream, peer_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("accept error (retrying): {e}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
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

/// Ping cadence, idle ceiling, and handshake/write timeouts so stalled or
/// half-open peers cannot leak tasks or hang indefinitely.
const PING_INTERVAL: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// One connection, one `sync::State`. Losing it only costs a fuller first
/// message on reconnect; the protocol re-converges.
async fn serve_peer(stream: TcpStream, handle: SyncHandle, changed_tx: watch::Sender<()>) -> Result<()> {
    let ws = tokio::time::timeout(HANDSHAKE_TIMEOUT, tokio_tungstenite::accept_async(stream))
        .await
        .context("WebSocket handshake timed out")??;
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
                        // Offload Automerge merge + disk save to `spawn_blocking`
                        // so `SyncHandle::lock()` + `rename_with_retry` never block tokio workers.
                        let h = handle.clone();
                        let (next_state, changed) = tokio::task::spawn_blocking(move || {
                            let res = apply_and_save(&h, &mut state, msg);
                            (state, res)
                        })
                        .await
                        .context("apply_and_save task panicked")?;
                        state = next_state;
                        if changed? {
                            let _ = changed_tx.send(());
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {} // pings/pongs/text: tungstenite answers pings
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
                send_ws(&mut ws_write, WsMessage::Ping(Vec::new().into())).await?;
            }
        }
        send_pending(&handle, &mut state, &mut ws_write).await?;
    }
}

/// Apply one message; save and return `true` only if heads moved or a prior save failed.
fn apply_and_save(handle: &SyncHandle, state: &mut SyncState, msg: SyncMessage) -> Result<bool> {
    let mut doc = handle.lock();
    if doc.is_read_only() {
        tracing::warn!("sync document is read-only; dropping an incoming change from a peer");
        return Ok(false);
    }
    let before = doc.heads();
    doc.doc_mut().sync().receive_sync_message(state, msg)?;
    let moved = doc.heads() != before;
    if !moved && !doc.has_save_failed() {
        return Ok(false);
    }
    doc.save().context("could not save the sync document")?;
    Ok(moved)
}

type WsSink = futures_util::stream::SplitSink<WebSocketStream<TcpStream>, WsMessage>;

async fn send_ws(ws_write: &mut WsSink, msg: WsMessage) -> Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, ws_write.send(msg))
        .await
        .context("WebSocket write timed out")??;
    Ok(())
}

async fn send_pending(handle: &SyncHandle, state: &mut SyncState, ws_write: &mut WsSink) -> Result<()> {
    let h = handle.clone();
    let mut s = std::mem::replace(state, SyncState::new());
    let (next_state, msg) = tokio::task::spawn_blocking(move || {
        let mut doc = h.lock();
        let msg = doc.doc_mut().sync().generate_sync_message(&mut s);
        (s, msg)
    })
    .await
    .context("generate_pending task panicked")?;
    *state = next_state;
    if let Some(msg) = msg {
        send_ws(ws_write, WsMessage::Binary(msg.encode().into())).await?;
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
        // Tailnet-range (100.64.0.0/10): looks private, reachable tailnet-wide.
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

    /// A fresh server and a fresh client must start from the same genesis, or
    /// their first sync re-introduces the whole-map conflict genesis prevents.
    #[test]
    fn a_fresh_server_document_starts_from_the_same_genesis_a_client_would() {
        let dir = std::env::temp_dir().join(format!("beamer_sync_server_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let (mut server_doc, load_error) = SyncDoc::open(notes_document_path(&dir));
        assert!(load_error.is_none());

        let mut client_doc = sync_doc::new_document();
        assert_eq!(server_doc.heads(), client_doc.get_heads());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

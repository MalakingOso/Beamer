#![cfg(not(target_os = "windows"))]

//! Focused-window identification via the Beamer GNOME Shell extension.
//!
//! On GNOME Wayland the compositor does not expose focused-window metadata
//! to unprivileged clients, so we ship a minimal Shell extension that
//! exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` over D-Bus.
//! This module is the client side of that interface, and it also owns the
//! cached session-bus connection shared by every Beamer helper call
//! (`gnome.rs`'s `TypeText`/`GetVersion`/`SendPasteChord` included).

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Cached session-bus connection, shared by every D-Bus call this process
/// makes to the Beamer Shell-extension helper. Building a
/// `zbus::blocking::Connection` performs a full handshake with the session
/// bus (socket connect + SASL auth + `Hello()`) — too expensive to redo on
/// every injection — so it's built once and cloned from then on (zbus
/// connections are cheap `Arc`-backed handles; cloning is not a new
/// handshake). `Mutex<Option<_>>` rather than `OnceLock` because a dead
/// connection (closed socket) must be replaceable, not permanent.
static CONN: Mutex<Option<zbus::blocking::Connection>> = Mutex::new(None);

/// Get the cached connection, building one if this is the first call (or
/// the previous one was dropped after a connection-level failure).
fn cached_conn() -> Result<zbus::blocking::Connection, zbus::Error> {
    let mut slot = CONN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(conn) = slot.as_ref() {
        return Ok(conn.clone());
    }
    let conn = zbus::blocking::connection::Builder::session()?.build()?;
    *slot = Some(conn.clone());
    Ok(conn)
}

/// Drop the cached connection so the next call rebuilds it from scratch.
/// Only call this after a connection-level failure (dead socket) — never
/// after a method-level failure (extension absent, wrong version, or just
/// slow to reply), which says nothing about the health of the bus
/// connection itself.
fn drop_cached_conn() {
    let mut slot = CONN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = None;
}

/// True when `err` means the cached connection's underlying socket is dead
/// and a reconnect is warranted, as opposed to a method-level failure
/// (extension not installed, wrong version, `ServiceUnknown`/`NoReply`/
/// `UnknownMethod`, or a plain call timeout) where the bus connection
/// itself is still perfectly healthy. Conservative by design: anything not
/// unambiguously a dead socket is left alone, so a merely-absent or slow
/// extension never discards a good connection. Notably this excludes
/// `ErrorKind::TimedOut` — our own per-call timeout (`with_timeout` below)
/// surfaces as `InputOutput` too, but a slow reply says nothing about
/// socket health and must not trigger a reconnect+retry loop.
fn is_connection_dead(err: &zbus::Error) -> bool {
    matches!(
        err,
        zbus::Error::InputOutput(io_err) if matches!(
            io_err.kind(),
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::UnexpectedEof
        )
    )
}

/// Run a D-Bus call on a detached thread, enforcing `timeout_ms` from the
/// caller's side. zbus bakes `method_timeout` into a `Connection` at build
/// time, so it can't vary call-to-call on a *shared/cached* connection the
/// way the old "build a fresh connection every call" code effectively could
/// (each call got its own connection built with exactly the timeout it
/// wanted). Call sites here need different timeouts against the SAME cached
/// connection — a 100 ms focus poll vs. a multi-second `TypeText` — so the
/// timeout is enforced here instead. If `call` hasn't finished when the
/// deadline passes, this returns a timeout error and abandons the thread;
/// `call` finishes on its own later and its result is silently discarded
/// (the `tx.send` on a dropped receiver just fails).
fn with_timeout<T, F>(timeout_ms: u64, call: F) -> Result<T, zbus::Error>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, zbus::Error> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("beamer-dbus-call".into())
        .spawn(move || {
            let _ = tx.send(call());
        });
    if let Err(e) = spawned {
        return Err(zbus::Error::InputOutput(Arc::new(std::io::Error::other(format!(
            "beamer: failed to spawn D-Bus call thread: {e}"
        )))));
    }
    rx.recv_timeout(Duration::from_millis(timeout_ms)).unwrap_or_else(|_| {
        Err(zbus::Error::InputOutput(Arc::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "beamer: D-Bus helper call timed out",
        ))))
    })
}

/// Call a method on the Beamer Shell-extension helper interface
/// (`app.beamer.FocusProvider` on `org.gnome.Shell`), reusing the cached
/// session-bus connection. `timeout_ms` bounds this one call. On a
/// connection-level failure the cached connection is dropped and the call
/// is retried exactly once against a freshly-built one — a genuinely dead
/// connection can't recover on its own, and retrying more than once would
/// risk looping against a session bus that's simply gone.
pub(crate) fn call_helper<A, R>(timeout_ms: u64, method: &'static str, args: A) -> Result<R, zbus::Error>
where
    A: serde::Serialize + zbus::zvariant::DynamicType + Clone + Send + 'static,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d> + Send + 'static,
{
    match try_call_helper(timeout_ms, method, args.clone()) {
        Ok(v) => Ok(v),
        Err(e) if is_connection_dead(&e) => {
            tracing::debug!(
                "focus: cached D-Bus connection looks dead ({}) — reconnecting and retrying {}",
                e,
                method
            );
            drop_cached_conn();
            try_call_helper(timeout_ms, method, args)
        }
        Err(e) => Err(e),
    }
}

fn try_call_helper<A, R>(timeout_ms: u64, method: &'static str, args: A) -> Result<R, zbus::Error>
where
    A: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d> + Send + 'static,
{
    let conn = cached_conn()?;
    with_timeout(timeout_ms, move || {
        let proxy = zbus::blocking::Proxy::new(
            &conn,
            "org.gnome.Shell",
            "/app/beamer/FocusProvider",
            "app.beamer.FocusProvider",
        )?;
        proxy.call(method, &args)
    })
}

const TERMINAL_APP_IDS: &[&str] = &[
    "org.gnome.console",
    "org.gnome.terminal",
    "org.wezfurlong.wezterm",
    "dev.warp.warp",
    "com.mitchellh.ghostty",
    "kitty",
    "foot",
    "alacritty",
    "org.kde.konsole",
    "io.elementary.terminal",
    "org.xfce.terminal",
    "com.raggesilver.blackbox",
    "tilix",
];

/// Exact-match against a curated list, case-insensitive. No substring
/// heuristic — "thunderbird" contains "term" but is not a terminal.
pub fn is_terminal(app_id: &str) -> bool {
    TERMINAL_APP_IDS.iter().any(|id| app_id.eq_ignore_ascii_case(id))
}

fn call_extension() -> Result<String, zbus::Error> {
    call_helper(100, "GetFocusedAppId", ())
}

/// Returns the focused window's app id, lowercased. Returns `None` if the
/// extension isn't installed/enabled, the D-Bus call fails, or no window
/// is focused. Every failure path is silent (debug-logged) so callers can
/// treat `None` as "unknown, fall back to defaults".
pub fn focused_app_id() -> Option<String> {
    map_call_result(call_extension())
}

fn map_call_result(result: Result<String, zbus::Error>) -> Option<String> {
    match result {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s.to_ascii_lowercase()),
        Err(e) => {
            tracing::debug!("focus: D-Bus call failed: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_app_ids_match_case_insensitive() {
        assert!(is_terminal("org.wezfurlong.wezterm"));
        assert!(is_terminal("Kitty"));
        assert!(is_terminal("FOOT"));
        assert!(is_terminal("org.gnome.Console"));
    }

    #[test]
    fn non_terminal_app_ids_do_not_match() {
        assert!(!is_terminal("firefox"));
        assert!(!is_terminal("code"));
        assert!(!is_terminal("org.mozilla.firefox"));
        assert!(!is_terminal(""));
        assert!(!is_terminal("thunderbird")); // contains "term"; must not match
    }

    #[test]
    fn empty_string_maps_to_none() {
        let got = map_call_result(Ok(String::new()));
        assert_eq!(got, None);
    }

    #[test]
    fn non_empty_string_is_lowercased() {
        let got = map_call_result(Ok("Org.WezFurlong.WezTerm".into()));
        assert_eq!(got.as_deref(), Some("org.wezfurlong.wezterm"));
    }

    #[test]
    fn dbus_error_maps_to_none() {
        let err = zbus::Error::Failure("simulated".into());
        let got = map_call_result(Err(err));
        assert_eq!(got, None);
    }

    fn io_err(kind: std::io::ErrorKind) -> zbus::Error {
        zbus::Error::InputOutput(Arc::new(std::io::Error::new(kind, "simulated")))
    }

    #[test]
    fn dead_socket_errors_are_connection_dead() {
        assert!(is_connection_dead(&io_err(std::io::ErrorKind::BrokenPipe)));
        assert!(is_connection_dead(&io_err(std::io::ErrorKind::ConnectionReset)));
        assert!(is_connection_dead(&io_err(std::io::ErrorKind::ConnectionAborted)));
        assert!(is_connection_dead(&io_err(std::io::ErrorKind::NotConnected)));
        assert!(is_connection_dead(&io_err(std::io::ErrorKind::UnexpectedEof)));
    }

    #[test]
    fn timeout_is_not_connection_dead() {
        // A slow/absent extension surfaces as our own per-call TimedOut —
        // that says nothing about the health of the session-bus socket and
        // must never trigger a reconnect.
        assert!(!is_connection_dead(&io_err(std::io::ErrorKind::TimedOut)));
    }

    #[test]
    fn method_level_errors_are_not_connection_dead() {
        // Non-InputOutput variants (what a ServiceUnknown/NoReply/
        // UnknownMethod reply from an absent or older extension surfaces
        // as) must never be treated as a dead socket.
        assert!(!is_connection_dead(&zbus::Error::Failure("simulated".into())));
        assert!(!is_connection_dead(&zbus::Error::InterfaceNotFound));
    }

    #[test]
    fn with_timeout_returns_ok_when_call_finishes_in_time() {
        let got = with_timeout(1000, || Ok::<_, zbus::Error>(42));
        assert_eq!(got.unwrap(), 42);
    }

    #[test]
    fn with_timeout_returns_timed_out_when_call_is_slow() {
        let start = std::time::Instant::now();
        let got = with_timeout(20, || {
            std::thread::sleep(Duration::from_millis(500));
            Ok::<_, zbus::Error>(())
        });
        assert!(start.elapsed() < Duration::from_millis(400), "should not block on the slow call");
        match got {
            Err(zbus::Error::InputOutput(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::TimedOut)
            }
            other => panic!("expected a TimedOut InputOutput error, got {other:?}"),
        }
    }

    #[test]
    fn with_timeout_propagates_the_call_error() {
        let got = with_timeout(1000, || Err::<(), _>(zbus::Error::Failure("boom".into())));
        assert!(matches!(got, Err(zbus::Error::Failure(msg)) if msg == "boom"));
    }
}

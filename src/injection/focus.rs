#![cfg(not(target_os = "windows"))]

//! Focused-window lookup via the Beamer GNOME Shell extension.
//!
//! GNOME Wayland hides focused-window metadata from clients, so the extension
//! exports `app.beamer.FocusProvider.GetFocusedAppId()` over D-Bus. This
//! module is the client side, plus the cached session-bus connection shared
//! by all helper calls (`TypeText`/`GetVersion`/`SendPasteChord`).

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Cached session-bus connection for all helper D-Bus calls. Built once and
/// cloned after (zbus handles are cheap `Arc`s; cloning skips the handshake).
/// `Mutex<Option<_>>` (not `OnceLock`) so a dead connection can be replaced.
/// Separate from the shell indicator's cache (different thread/timeout needs).
static CONN: Mutex<Option<zbus::blocking::Connection>> = Mutex::new(None);

fn cached_conn() -> Result<zbus::blocking::Connection, zbus::Error> {
    let mut slot = CONN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(conn) = slot.as_ref() {
        return Ok(conn.clone());
    }
    let conn = zbus::blocking::connection::Builder::session()?.build()?;
    *slot = Some(conn.clone());
    Ok(conn)
}

/// Drop the cached connection. Only after a dead socket — never after a
/// method-level failure (absent/slow extension), which says nothing about bus health.
fn drop_cached_conn() {
    let mut slot = CONN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = None;
}

/// True only for a dead socket (reconnect warranted). Method-level failures
/// (absent extension, wrong version) and our own `TimedOut` never count —
/// a slow reply says nothing about socket health.
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

/// Run a D-Bus call with a per-call timeout. zbus bakes its timeout into the
/// connection at build time, so it can't vary across calls sharing this cached
/// connection (100 ms focus poll vs. multi-second `TypeText`). A timed-out call
/// is abandoned; its late result is discarded.
///
/// Known cost: the worker thread stays parked inside the blocking zbus call
/// until it returns, so each timeout leaks one thread until the call itself
/// finishes. Timeouts are rare (a wedged helper, not steady state) and every
/// call site bounds its wait, so accumulation is self-limiting; killing the
/// thread is unsound and an async rewrite of this module is not worth it
/// for that bound.
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

/// Call a helper method over the cached connection. Retried exactly once
/// on a dead socket against a fresh connection; never retried otherwise.
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
    "org.gnome.terminal-server",
    "gnome-terminal-server",
    "org.wezfurlong.wezterm",
    "wezterm",
    "dev.warp.warp",
    "com.mitchellh.ghostty",
    "ghostty",
    "kitty",
    "foot",
    "alacritty",
    "org.kde.konsole",
    "konsole",
    "io.elementary.terminal",
    "org.xfce.terminal",
    "com.raggesilver.blackbox",
    "tilix",
    "app.ptyxis.ptyxis",
    "ptyxis",
    "xterm",
    "uxterm",
];

/// Exact-match against a curated list, case-insensitive. No substring
/// heuristic — "thunderbird" contains "term" but is not a terminal.
pub fn is_terminal(app_id: &str) -> bool {
    TERMINAL_APP_IDS.iter().any(|id| app_id.eq_ignore_ascii_case(id))
}

/// True when the app id is Beamer itself. Substring, unlike `is_terminal`:
/// our Wayland `app_id` is pinned to `beamer` (`set_gtk_prgname`), the bundle
/// id is `com.beamer.app`, and the extension matches the same way
/// (`id.includes('beamer')`). Nothing else on a desktop contains it.
pub fn is_self(app_id: &str) -> bool {
    app_id.to_ascii_lowercase().contains("beamer")
}

fn call_extension() -> Result<String, zbus::Error> {
    call_helper(100, "GetFocusedAppId", ())
}

/// Focused app id, lowercased. `None` means unknown (no extension, call
/// failed, nothing focused) — callers fall back to defaults.
pub fn focused_app_id() -> Option<String> {
    map_call_result(call_extension())
}

/// True when the focused window is one of ours. `None` (unknown focus) is
/// not self — an unattributed target keeps the normal chain.
pub fn focused_is_self() -> bool {
    focused_app_id().is_some_and(|id| is_self(&id))
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
    fn bare_x11_class_names_and_new_terminals_match() {
        // XWayland exposes WM_CLASS, not app ids.
        assert!(is_terminal("wezterm"));
        assert!(is_terminal("ghostty"));
        assert!(is_terminal("konsole"));
        assert!(is_terminal("xterm"));
        assert!(is_terminal("uxterm"));
        assert!(is_terminal("gnome-terminal-server"));
        assert!(is_terminal("app.ptyxis.Ptyxis"));
        assert!(is_terminal("ptyxis"));
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
    fn beamer_own_app_ids_count_as_self() {
        assert!(is_self("beamer"));
        assert!(is_self("Beamer"));
        assert!(is_self("com.beamer.app"));
    }

    #[test]
    fn other_app_ids_are_not_self() {
        assert!(!is_self("firefox"));
        assert!(!is_self("code"));
        assert!(!is_self("org.gnome.Console"));
        assert!(!is_self("thunderbird"));
        assert!(!is_self(""));
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
        // A slow reply must never trigger a reconnect.
        assert!(!is_connection_dead(&io_err(std::io::ErrorKind::TimedOut)));
    }

    #[test]
    fn method_level_errors_are_not_connection_dead() {
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

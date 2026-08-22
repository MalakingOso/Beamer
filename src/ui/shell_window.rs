#![cfg(target_os = "linux")]

//! Client for the GNOME Shell extension's window placement (extension v5).
//!
//! **Why this exists at all.** Core Wayland and `xdg-shell` give a client no
//! way to learn or set its own absolute position — the compositor owns
//! placement by design. `tao::Window::outer_position()` does not error under
//! Wayland, it returns `Ok((0, 0))`: it reads a cached atomic fed by GDK's
//! `frame_extents()` on `configure_event`, and GDK has no global coordinates
//! there. Believing it silently persists garbage, so it is never called.
//!
//! Code running *inside* GNOME Shell is not a Wayland client and is bound by
//! none of that. Beamer already ships an extension for text injection and the
//! recording pill, so placing a window costs one more D-Bus method rather than
//! a new dependency.
//!
//! Calls degrade to a silent no-op against an older helper: a missing method
//! comes back as a D-Bus error, and a note simply lands wherever Mutter chose.
//!
//! Follows `shell_indicator`'s `zbus::blocking` pattern, with one difference —
//! placement is rare and bursty rather than a steady ~15 Hz stream, so each
//! call gets its own short-lived thread instead of a long-running worker.

use std::time::Duration;

pub const DBUS_DEST: &str = "org.gnome.Shell";
pub const DBUS_PATH: &str = "/app/beamer/FocusProvider";
pub const DBUS_IFACE: &str = "app.beamer.FocusProvider";

/// How long to wait before each retry after the first attempt.
///
/// Beamer cannot observe the Wayland map event: `new_window().await` resolves
/// when the webview has been constructed, not when Mutter has mapped the window
/// and given it a title. So the first `PlaceWindow` usually finds nothing and
/// has to be retried while the window appears. The schedule is front-loaded so
/// a window that maps quickly — the common case — is placed quickly, and backs
/// off rather than hammering the shell for the full window.
const RETRY_DELAYS: [Duration; 6] = [
    Duration::from_millis(100),
    Duration::from_millis(150),
    Duration::from_millis(250),
    Duration::from_millis(350),
    Duration::from_millis(500),
    Duration::from_millis(650),
];

fn retry_delays() -> &'static [Duration] {
    &RETRY_DELAYS
}

/// Ask the shell to move a note's window and pin it across workspaces.
///
/// Fire-and-forget: the caller is on the UI thread and placement is advisory.
/// Pass `(-1, -1)` to change only the sticky state, leaving the position alone.
pub fn place(title: String, x: i32, y: i32, all_workspaces: bool) {
    let spawned = std::thread::Builder::new()
        .name("beamer-place-window".into())
        .spawn(move || {
            if place_blocking(&title, x, y, all_workspaces) {
                tracing::debug!("Placed {} at ({}, {})", title, x, y);
            } else {
                tracing::debug!("Could not place {} — left where Mutter put it", title);
            }
        });
    if let Err(e) = spawned {
        tracing::debug!("place: could not spawn worker: {}", e);
    }
}

fn place_blocking(title: &str, x: i32, y: i32, all_workspaces: bool) -> bool {
    let proxy = match connect() {
        Some(p) => p,
        None => return false,
    };

    let mut delays = retry_delays().iter();
    loop {
        match proxy.call::<_, _, bool>("PlaceWindow", &(title, x, y, all_workspaces)) {
            // The window exists and has been moved.
            Ok(true) => return true,
            // The helper is running but no window carries this title yet —
            // it has not been mapped. Worth waiting for.
            Ok(false) => {}
            // No helper, or a helper too old to know the method. Retrying
            // cannot change either, so give up immediately rather than
            // blocking a thread for two seconds on a foregone conclusion.
            Err(e) => {
                tracing::debug!("PlaceWindow unavailable: {}", e);
                return false;
            }
        }
        match delays.next() {
            Some(d) => std::thread::sleep(*d),
            None => return false,
        }
    }
}

fn connect() -> Option<zbus::blocking::Proxy<'static>> {
    zbus::blocking::connection::Builder::session()
        .map(|b| b.method_timeout(Duration::from_millis(500)))
        .and_then(|b| b.build())
        .and_then(|conn| zbus::blocking::Proxy::new(&conn, DBUS_DEST, DBUS_PATH, DBUS_IFACE))
        .map_err(|e| tracing::debug!("place: session bus unavailable: {}", e))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_schedule_covers_a_slow_window_map_without_hammering() {
        let delays = retry_delays();
        assert!(!delays.is_empty(), "one attempt is not enough — the first call almost always misses");

        let total: Duration = delays.iter().sum();
        assert!(
            total >= Duration::from_millis(1500) && total <= Duration::from_millis(2500),
            "retry window is {total:?}; under ~1.5s loses slow maps, over ~2.5s keeps a thread alive long after the user has moved on"
        );

        assert!(
            delays.windows(2).all(|w| w[0] <= w[1]),
            "delays must not decrease — the point of backing off is to stop hammering the shell"
        );
    }

    #[test]
    fn dbus_address_matches_the_extension() {
        // Pinned because the extension is the other half of this contract and
        // cannot be re-tested without another full GNOME log out. A typo here
        // fails silently: every call errors, every note lands wherever Mutter
        // chose, and nothing in the app says why.
        assert_eq!(DBUS_DEST, "org.gnome.Shell");
        assert_eq!(DBUS_PATH, "/app/beamer/FocusProvider");
        assert_eq!(DBUS_IFACE, "app.beamer.FocusProvider");
    }
}

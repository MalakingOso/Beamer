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
//! ⚠️ **`PlaceWindow` returning `true` does not mean the window stayed put.**
//! It means a window with that title was found and `move_frame` was called on
//! it. Mutter applies its *own* initial placement when a window is first
//! shown, and that happens after the window is already findable by title — so
//! an early call is accepted, logged as a success, and then silently
//! overwritten by Mutter's cascade. Measured 2026-08-25: three notes asked for
//! (2311, 508), (3389, 1036) and (1648, 584) all reported placed, and all three
//! were actually at (1120, 590) + 50px per note — the cascade, which is exactly
//! the clustering the scatter exists to prevent. Re-issuing the same call
//! against the settled window moved it correctly, so `move_frame` was never the
//! problem; believing the first `true` was.
//!
//! Hence the loop below does not stop at `true`. It re-reads the frame through
//! `GetWindowFrame` after a delay and only believes a placement that is still
//! there — the delay is the load-bearing part, since a read taken immediately
//! would confirm a position Mutter has not clobbered *yet*.
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
///
/// The tail exists for the startup burst specifically. Restoring a session
/// opens every note in one reconcile pass, so several heavy webviews map at
/// once and the last of them can take far longer than a note dictated into an
/// already-running app. A budget tuned to the quiet case expires silently and
/// leaves exactly the clustering this schedule is meant to prevent.
const RETRY_DELAYS: [Duration; 9] = [
    Duration::from_millis(100),
    Duration::from_millis(150),
    Duration::from_millis(250),
    Duration::from_millis(350),
    Duration::from_millis(500),
    Duration::from_millis(650),
    Duration::from_millis(800),
    Duration::from_millis(1000),
    Duration::from_millis(1200),
];

fn retry_delays() -> &'static [Duration] {
    &RETRY_DELAYS
}

/// How long a placement must survive before it is believed.
///
/// Verification is worthless without it. Mutter's initial placement lands some
/// unspecified time after the window becomes findable by title, so a frame read
/// taken straight after a successful `move_frame` can match, be believed, and
/// then be overwritten a moment later. Waiting past the point where the window
/// is settled is the only thing that makes a matching read mean anything.
///
/// Half a second is chosen against the observed timeline — the clobber followed
/// a call issued 10ms after the window appeared — and is comfortably inside the
/// retry budget, so several attempts still remain after it elapses. It costs
/// nothing visible: the window is already in the right place by then, this only
/// delays the worker thread agreeing that it is.
const SETTLE: Duration = Duration::from_millis(500);

/// How far a window may sit from where it was asked to go and still count.
///
/// `move_frame` and `get_frame_rect` both work on the frame rect, and observed
/// round-trips have been exact. A couple of pixels of slack costs nothing and
/// avoids a rounding difference turning into an infinite re-place.
const TOLERANCE: i32 = 2;

/// How a placement attempt ended.
///
/// The two failures are deliberately distinct. Collapsing them into one `false`
/// is what made placement failure unreadable: "the extension is missing" and
/// "this particular window never appeared in time" call for opposite responses,
/// and a log line that cannot tell them apart sends you to the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// The window was moved, and was still there when re-read after `SETTLE`.
    Moved,
    /// The budget ran out. Carries the last frame position actually observed,
    /// which is the difference between "the window never appeared" (`None`) and
    /// "something keeps moving it back" — two failures that need opposite
    /// investigations and used to produce the same silence.
    TimedOut(Option<(i32, i32)>),
    /// No helper on the bus, or one too old to know the method.
    NoHelper,
}

/// Ask the shell to move a note's window and pin it across workspaces.
///
/// Fire-and-forget: the caller is on the UI thread and placement is advisory.
/// Pass `(-1, -1)` to change only the sticky state, leaving the position alone.
pub fn place(title: String, x: i32, y: i32, all_workspaces: bool) {
    let spawned = std::thread::Builder::new()
        .name("beamer-place-window".into())
        .spawn(move || match place_blocking(&title, x, y, all_workspaces) {
            Placement::Moved => tracing::debug!("Placed {} at ({}, {})", title, x, y),
            // Warn, not debug. This is the failure that produces a pile of
            // notes on top of each other, and at default log level the old
            // `debug!` made it indistinguishable from the layout simply being
            // wrong — so it was diagnosed as a scatter bug for as long as it
            // went unseen.
            Placement::TimedOut(None) => tracing::warn!(
                "Could not place {} at ({}, {}) — no window by that title within {:?}; \
                 it is wherever Mutter put it",
                title,
                x,
                y,
                retry_delays().iter().sum::<Duration>(),
            ),
            Placement::TimedOut(Some((fx, fy))) => tracing::warn!(
                "Could not place {} at ({}, {}) — it keeps returning to ({}, {}) \
                 despite {:?} of retries; something else is placing it",
                title,
                x,
                y,
                fx,
                fy,
                retry_delays().iter().sum::<Duration>(),
            ),
            Placement::NoHelper => tracing::warn!(
                "Could not place {} at ({}, {}) — the GNOME helper is absent or too old \
                 (log out and back in after an extension update)",
                title,
                x,
                y,
            ),
        });
    if let Err(e) = spawned {
        tracing::warn!("place: could not spawn worker: {}", e);
    }
}

fn place_blocking(title: &str, x: i32, y: i32, all_workspaces: bool) -> Placement {
    let proxy = match connect() {
        Some(p) => p,
        None => return Placement::NoHelper,
    };

    // `(-1, -1)` means "only change the sticky state" — the extension skips
    // `move_frame` entirely, so there is no position to read back and nothing
    // for the verification below to say. Waiting five seconds to confirm a
    // move that was never requested would be pure delay.
    let positioned = x >= 0 || y >= 0;

    let started = std::time::Instant::now();
    let mut last_seen: Option<(i32, i32)> = None;
    let mut delays = retry_delays().iter();
    loop {
        match proxy.call::<_, _, bool>("PlaceWindow", &(title, x, y, all_workspaces)) {
            // A window by this title exists and `move_frame` has been called on
            // it. Deliberately NOT treated as success when a position was
            // asked for — see the module docs: Mutter's own initial placement
            // arrives later and overwrites it.
            Ok(true) if !positioned => return Placement::Moved,
            Ok(true) => {}
            // The helper is running but no window carries this title yet —
            // it has not been mapped. Worth waiting for.
            Ok(false) => {}
            // No helper, or a helper too old to know the method. Retrying
            // cannot change either, so give up immediately rather than
            // blocking a thread for five seconds on a foregone conclusion.
            Err(e) => {
                tracing::debug!("PlaceWindow unavailable: {}", e);
                return Placement::NoHelper;
            }
        }

        match delays.next() {
            Some(d) => std::thread::sleep(*d),
            None => return Placement::TimedOut(last_seen),
        }

        // Verify only once the window has had time to settle. Before that a
        // match proves nothing — the clobber may simply not have happened yet.
        if started.elapsed() < SETTLE {
            continue;
        }
        match proxy.call::<_, _, (bool, i32, i32, u32, u32)>("GetWindowFrame", &(title,)) {
            Ok((true, fx, fy, _, _)) => {
                last_seen = Some((fx, fy));
                if (fx - x).abs() <= TOLERANCE && (fy - y).abs() <= TOLERANCE {
                    return Placement::Moved;
                }
                tracing::debug!(
                    "{} is at ({}, {}) not ({}, {}) — placing again",
                    title,
                    fx,
                    fy,
                    x,
                    y
                );
            }
            // Still unmapped. The next `PlaceWindow` will find it or not.
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("GetWindowFrame unavailable: {}", e);
                return Placement::NoHelper;
            }
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
            total >= Duration::from_millis(4500) && total <= Duration::from_millis(5500),
            "retry window is {total:?}; under ~4.5s loses the last window of a startup burst — \
             every restored note maps at once — and over ~5.5s keeps a thread alive long after \
             the user has moved on"
        );

        assert!(
            delays.windows(2).all(|w| w[0] <= w[1]),
            "delays must not decrease — the point of backing off is to stop hammering the shell"
        );
    }

    #[test]
    fn the_settle_delay_leaves_room_to_actually_retry() {
        // `SETTLE` suppresses verification, so a value too close to the total
        // budget would leave one confirmation attempt or none — and a single
        // attempt cannot re-place a window that Mutter clobbers, which is the
        // whole reason the loop verifies.
        let delays = retry_delays();
        let total: Duration = delays.iter().sum();
        assert!(SETTLE < total / 2, "SETTLE {SETTLE:?} eats the {total:?} budget");

        let mut elapsed = Duration::ZERO;
        let attempts = delays
            .iter()
            .filter(|d| {
                elapsed += **d;
                elapsed >= SETTLE
            })
            .count();
        assert!(
            attempts >= 3,
            "only {attempts} verification attempts survive SETTLE — a clobber \
             arriving late would go uncorrected"
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

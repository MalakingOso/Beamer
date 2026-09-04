#![cfg(target_os = "linux")]

//! Client for the GNOME Shell extension's window placement. Wayland gives a
//! client no position control (`outer_position()` returns a cached `(0, 0)`),
//! so the in-shell extension moves windows over D-Bus; missing methods degrade
//! to a no-op landing wherever Mutter chose. ⚠️ `PlaceWindow` returning `true`
//! doesn't mean the window stayed put — Mutter's own initial placement lands
//! later and clobbers early calls — so the loop re-reads the frame after a
//! settle delay and only believes a placement that survives it.

use std::time::Duration;

pub const DBUS_DEST: &str = "org.gnome.Shell";
pub const DBUS_PATH: &str = "/app/beamer/FocusProvider";
pub const DBUS_IFACE: &str = "app.beamer.FocusProvider";

/// Retry schedule. `new_window` resolves at webview construction, not at map, so
/// the first `PlaceWindow` usually finds nothing. Front-loaded for the fast
/// common case; the long tail covers the startup burst, where every restored
/// note maps at once and the last one is slow.
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

/// How long a placement must survive to be believed. Mutter clobbers settled
/// positions, so a frame read taken too early can match and then be overwritten.
const SETTLE: Duration = Duration::from_millis(500);

/// Slack between asked and observed position (guards rounding loops).
const TOLERANCE: i32 = 2;

/// How a placement attempt ended. The failures stay distinct: "no helper" vs
/// "window never appeared" vs "something keeps moving it back" need opposite fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// Moved, and still there when re-read after `SETTLE`.
    Moved,
    /// Budget ran out; carries the last observed frame (`None` = never appeared).
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
            // Warn: a pile of notes here reads as a scatter bug otherwise.
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

    // `(-1, -1)` = sticky-state only; the extension skips `move_frame`, so no verify.
    let positioned = x >= 0 || y >= 0;

    let started = std::time::Instant::now();
    let mut last_seen: Option<(i32, i32)> = None;
    let mut delays = retry_delays().iter();
    loop {
        match proxy.call::<_, _, bool>("PlaceWindow", &(title, x, y, all_workspaces)) {
            // NOT success when positioned: Mutter's placement arrives later and clobbers it.
            Ok(true) if !positioned => return Placement::Moved,
            Ok(true) => {}
            // No window by this title yet (unmapped). Worth waiting for.
            Ok(false) => {}
            // No helper or too old — retrying can't help, give up at once.
            Err(e) => {
                tracing::debug!("PlaceWindow unavailable: {}", e);
                return Placement::NoHelper;
            }
        }

        match delays.next() {
            Some(d) => std::thread::sleep(*d),
            None => return Placement::TimedOut(last_seen),
        }

        // Verify only after SETTLE — an earlier match may predate the clobber.
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
        // A typo fails silently (every call errors, notes land wherever Mutter chose).
        assert_eq!(DBUS_DEST, "org.gnome.Shell");
        assert_eq!(DBUS_PATH, "/app/beamer/FocusProvider");
        assert_eq!(DBUS_IFACE, "app.beamer.FocusProvider");
    }
}

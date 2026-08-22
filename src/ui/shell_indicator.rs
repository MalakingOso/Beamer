#![cfg(target_os = "linux")]

//! Client for the GNOME Shell extension's recording pill (extension v2).
//!
//! All D-Bus traffic runs on one dedicated worker thread fed by a channel, so
//! UI code can fire-and-forget from any context. Level updates are coalesced
//! (only the newest matters) and every call degrades to a silent no-op when
//! the helper extension isn't active — non-GNOME desktops keep the tray-icon
//! swap as their only indicator.

use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::Duration;

enum Cmd {
    Show(&'static str),
    Level(f32),
    Hide,
}

pub fn show(state: &'static str) {
    let _ = sender().send(Cmd::Show(state));
}

pub fn update_level(level: f32) {
    let _ = sender().send(Cmd::Level(level));
}

pub fn hide() {
    let _ = sender().send(Cmd::Hide);
}

fn sender() -> &'static Sender<Cmd> {
    static SENDER: OnceLock<Sender<Cmd>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("beamer-shell-indicator".into())
            .spawn(move || worker(rx))
            .expect("failed to spawn shell-indicator worker");
        tx
    })
}

struct Worker {
    /// This worker's own cached D-Bus proxy/connection, built lazily in
    /// `proxy()` below with a fixed 200ms method timeout sized for the
    /// ~15Hz level-update cadence this worker drives.
    ///
    /// `src/injection/focus.rs` keeps a separate cached session-bus
    /// connection (`CONN`) for its own D-Bus calls (focus lookup,
    /// TypeText/SendPasteChord). The two caches are deliberately not
    /// unified: this one lives on a single dedicated worker thread with a
    /// baked-in timeout tuned for coalesced level updates, while
    /// `focus.rs`'s is shared across arbitrary callers each wanting their
    /// own per-call timeout — different enough usage patterns that sharing
    /// one cache would mean compromising both.
    proxy: Option<zbus::blocking::Proxy<'static>>,
    /// Whether the helper answered a v2 GetVersion at the last probe. Gates
    /// `Level` so a missing extension costs one probe per recording rather
    /// than 15 failed D-Bus calls a second — which is the whole reason the
    /// flag exists. See [`needs_probe`] for why `Hide` is not gated the same
    /// way.
    helper_active: bool,
}

/// Whether a command has to probe for the helper before it can be sent.
///
/// `Show` always probes: it starts an indicator session and its answer is what
/// gates everything after it.
///
/// `Hide` probes **when no session is active**, and that case is the whole
/// point. A `Hide` with no preceding `Show` is the stale-pill case: a previous
/// Beamer died without hiding the indicator — a crash, a SIGTERM, a `dx serve`
/// rebuild — and GNOME Shell is still drawing a pill for a recording that has
/// no process behind it. The indicator lives inside the shell, so nothing but
/// Beamer can clear it, and the only moment it can is startup, where
/// `helper_active` is false by construction. Dropping that `Hide` is exactly
/// what stranded the pill: `linux_integration`'s effect already sends one on
/// first render, and it went nowhere.
///
/// `Level` never probes. It is the 15Hz flood the gate was built for, and a
/// dropped level update costs one frame of a waveform.
fn needs_probe(cmd: &Cmd, helper_active: bool) -> bool {
    match cmd {
        Cmd::Show(_) => true,
        Cmd::Hide => !helper_active,
        Cmd::Level(_) => false,
    }
}

fn worker(rx: mpsc::Receiver<Cmd>) {
    let mut w = Worker { proxy: None, helper_active: false };

    while let Ok(mut cmd) = rx.recv() {
        // Coalesce queued level updates — only the latest is worth sending.
        while let Ok(next) = rx.try_recv() {
            match (&cmd, &next) {
                (Cmd::Level(_), Cmd::Level(_)) => cmd = next,
                _ => {
                    w.handle(cmd);
                    cmd = next;
                }
            }
        }
        w.handle(cmd);
    }
}

impl Worker {
    fn handle(&mut self, cmd: Cmd) {
        if needs_probe(&cmd, self.helper_active) {
            self.helper_active = self.probe_v2();
        }
        if !self.helper_active {
            return;
        }
        match cmd {
            Cmd::Show(state) => self.call("ShowIndicator", &(state)),
            Cmd::Level(level) => self.call("UpdateLevel", &(level as f64)),
            Cmd::Hide => self.call("HideIndicator", &()),
        }
    }

    fn proxy(&mut self) -> Option<&zbus::blocking::Proxy<'static>> {
        if self.proxy.is_none() {
            let built = zbus::blocking::connection::Builder::session()
                .map(|b| b.method_timeout(Duration::from_millis(200)))
                .and_then(|b| b.build())
                .and_then(|conn| {
                    zbus::blocking::Proxy::new(
                        &conn,
                        "org.gnome.Shell",
                        "/app/beamer/FocusProvider",
                        "app.beamer.FocusProvider",
                    )
                });
            match built {
                Ok(p) => self.proxy = Some(p),
                Err(e) => {
                    tracing::debug!("shell indicator: proxy unavailable: {}", e);
                }
            }
        }
        self.proxy.as_ref()
    }

    fn probe_v2(&mut self) -> bool {
        let Some(proxy) = self.proxy() else { return false };
        match proxy.call::<_, _, u32>("GetVersion", &()) {
            Ok(v) if v >= 2 => true,
            Ok(v) => {
                tracing::debug!("shell indicator: helper v{} has no indicator", v);
                false
            }
            Err(e) => {
                tracing::debug!("shell indicator: helper not active: {}", e);
                self.proxy = None; // rebuild next time
                false
            }
        }
    }

    fn call<A: serde::Serialize + zbus::zvariant::DynamicType>(
        &mut self,
        method: &str,
        args: &A,
    ) {
        let Some(proxy) = self.proxy() else { return };
        if let Err(e) = proxy.call::<_, _, ()>(method, args) {
            tracing::debug!("shell indicator: {} failed: {}", method, e);
            self.proxy = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hide_with_no_session_probes_rather_than_being_dropped() {
        // The stale-pill case. A previous Beamer died without hiding the
        // indicator, so GNOME Shell is still drawing one; the startup Hide
        // that clears it arrives with helper_active false, and skipping it
        // leaves the pill on screen with no error to explain it.
        assert!(needs_probe(&Cmd::Hide, false));
    }

    #[test]
    fn a_hide_that_ends_a_live_session_does_not_probe_again() {
        assert!(!needs_probe(&Cmd::Hide, true));
    }

    #[test]
    fn a_level_update_never_probes() {
        // Levels arrive at ~15Hz. Probing here is what the gate was built to
        // prevent, and a dropped level costs one frame of a waveform.
        assert!(!needs_probe(&Cmd::Level(0.5), false));
        assert!(!needs_probe(&Cmd::Level(0.5), true));
    }

    #[test]
    fn a_show_always_probes() {
        // Show opens the session, and its answer is what gates everything
        // after it — including whether the extension is there at all.
        assert!(needs_probe(&Cmd::Show("recording"), false));
        assert!(needs_probe(&Cmd::Show("recording"), true));
    }
}

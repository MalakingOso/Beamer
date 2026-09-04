#![cfg(target_os = "linux")]

//! Client for the GNOME Shell extension's recording pill. One worker thread fed
//! by a channel (fire-and-forget from anywhere); level updates coalesced to the
//! newest; silent no-op when the helper isn't active.

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
    /// Own cached proxy, 200ms timeout sized for the ~15Hz level cadence. Kept
    /// separate from `focus.rs`'s cache (different thread/timeout needs).
    proxy: Option<zbus::blocking::Proxy<'static>>,
    /// Whether the helper answered GetVersion at the last probe. Gates `Level`
    /// so a missing extension costs one probe per recording, not 15 failures/s.
    helper_active: bool,
}

/// Whether a command must probe first. `Show` always probes (it opens the
/// session). `Hide` probes when no session is active — the stale-pill case: a
/// previous Beamer died without hiding, and startup's `Hide` is the only thing
/// that can clear it. `Level` never probes (a drop costs one waveform frame).
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
        // Stale pill: skipping this leaves it on screen with no error.
        assert!(needs_probe(&Cmd::Hide, false));
    }

    #[test]
    fn a_hide_that_ends_a_live_session_does_not_probe_again() {
        assert!(!needs_probe(&Cmd::Hide, true));
    }

    #[test]
    fn a_level_update_never_probes() {
        assert!(!needs_probe(&Cmd::Level(0.5), false));
        assert!(!needs_probe(&Cmd::Level(0.5), true));
    }

    #[test]
    fn a_show_always_probes() {
        assert!(needs_probe(&Cmd::Show("recording"), false));
        assert!(needs_probe(&Cmd::Show("recording"), true));
    }
}

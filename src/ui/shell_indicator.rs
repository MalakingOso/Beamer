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
    /// Whether the helper answered a v2 GetVersion at the last Show. Gates
    /// Level/Hide so a missing extension costs one probe per recording, not
    /// 15 failed D-Bus calls per second.
    helper_active: bool,
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
        match cmd {
            Cmd::Show(state) => {
                self.helper_active = self.probe_v2();
                if self.helper_active {
                    self.call("ShowIndicator", &(state));
                }
            }
            Cmd::Level(level) => {
                if self.helper_active {
                    self.call("UpdateLevel", &(level as f64));
                }
            }
            Cmd::Hide => {
                if self.helper_active {
                    self.call("HideIndicator", &());
                }
            }
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

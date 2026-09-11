//! Desktop-level hotkey through the GNOME Shell extension (v7).
//!
//! The evdev listener reads `/dev/input`, and gnome-remote-desktop injects RDP
//! input inside Mutter through virtual devices that never appear there, so a
//! remoted-in user's chord never reaches evdev. The extension grabs the chord
//! with `Meta.Display.grab_accelerator` and re-emits press and release as
//! D-Bus signals. The grab consumes the chord, the same way `ll_hook.rs`
//! swallows it on Windows.
//!
//! Two threads, both on one session-bus connection: a worker that pushes the
//! chords (`SetHotkeys`), and a listener for the extension's signals. The
//! connection is never rebuilt, because the extension ties the grabs to the
//! caller's bus name and drops them when it vanishes. A new connection would
//! silently lose them.
//!
//! Absent or pre-v7 helper: nothing is owned and evdev behaves exactly as it
//! always has.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::linux_hotkey::{on_grab_event, release_desktop_bindings, HookState};
use super::{BindingConfig, HotkeyConfig, MAX_BINDINGS, VK_LWIN};

/// Helper version that provides `SetHotkeys` and the hotkey signals. The
/// injection floor (`REQUIRED_VERSION` in `injection/gnome.rs`) stays at 2:
/// an older helper still types, it just can't grab.
const HOTKEY_VERSION: u32 = 7;

const DEST: &str = "org.gnome.Shell";
const PATH: &str = "/app/beamer/FocusProvider";
const IFACE: &str = "app.beamer.FocusProvider";

/// Which binding slots the extension currently grabs; see `HookState`.
pub(super) type DesktopOwned = Arc<[AtomicBool; MAX_BINDINGS]>;

/// One accelerator per binding slot; empty = don't grab that slot.
type Accels = [String; MAX_BINDINGS];

enum Cmd {
    Push(Accels),
    HelperEnabled,
    HelperDisabled,
}

/// Sends chord changes to the grab worker. A no-op when the worker is gone.
#[derive(Clone)]
pub(super) struct GrabHandle(Sender<Cmd>);

impl GrabHandle {
    pub(super) fn push(&self, accels: Accels) {
        let _ = self.0.send(Cmd::Push(accels));
    }
}

/// GTK accelerator for a binding, as `grab_accelerator` parses it
/// (`<Control><Alt><Shift>` then an xkb keysym name).
///
/// `None` for a Super trigger: a bare Super grab collides with Mutter's
/// overlay key, so that binding always stays on evdev.
pub(super) fn hotkey_to_accelerator(cfg: &HotkeyConfig) -> Option<String> {
    if cfg.trigger_vk == VK_LWIN {
        return None;
    }
    let key = keysym_name(cfg.trigger_vk)?;
    let mut accel = String::new();
    if cfg.ctrl {
        accel.push_str("<Control>");
    }
    if cfg.alt {
        accel.push_str("<Alt>");
    }
    if cfg.shift {
        accel.push_str("<Shift>");
    }
    accel.push_str(&key);
    Some(accel)
}

/// xkb keysym name for every VK code `key_name_to_vk` can produce.
fn keysym_name(vk: u32) -> Option<String> {
    let name = match vk {
        0x20 => "space",
        0x0D => "Return",
        0x09 => "Tab",
        0x08 => "BackSpace",
        0x2E => "Delete",
        0x2D => "Insert",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "Page_Up",
        0x22 => "Page_Down",
        0x26 => "Up",
        0x28 => "Down",
        0x25 => "Left",
        0x27 => "Right",
        // Lowercase: the unshifted keysym, which is what the key resolves to.
        0x41..=0x5A => return char::from_u32(vk).map(|c| c.to_ascii_lowercase().to_string()),
        0x30..=0x39 => return char::from_u32(vk).map(String::from),
        0x70..=0x7B => return Some(format!("F{}", vk - 0x70 + 1)),
        _ => return None,
    };
    Some(name.to_string())
}

/// The per-slot accelerators for a binding list (slot = index).
pub(super) fn accelerators(bindings: &[BindingConfig]) -> Accels {
    std::array::from_fn(|i| {
        bindings
            .get(i)
            .and_then(|b| hotkey_to_accelerator(&b.config))
            .unwrap_or_default()
    })
}

/// Start the worker and push `initial`. The listener thread is started by
/// the worker once it has a connection.
pub(super) fn start(
    initial: Accels,
    state: Arc<Mutex<HookState>>,
    owned: DesktopOwned,
) -> GrabHandle {
    let (tx, rx) = mpsc::channel();
    let handle = GrabHandle(tx.clone());
    let spawned = std::thread::Builder::new()
        .name("beamer-hotkey-grab".into())
        .spawn(move || worker(rx, tx, state, owned));
    if let Err(e) = spawned {
        tracing::error!("Failed to spawn desktop hotkey worker: {}", e);
    }
    handle.push(initial);
    handle
}

fn connect() -> zbus::Result<zbus::blocking::Proxy<'static>> {
    let conn = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_millis(500))
        .build()?;
    zbus::blocking::Proxy::new(&conn, DEST, PATH, IFACE)
}

fn worker(rx: Receiver<Cmd>, tx: Sender<Cmd>, state: Arc<Mutex<HookState>>, owned: DesktopOwned) {
    let proxy = match connect() {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("desktop hotkey: no session bus, evdev only: {}", e);
            return;
        }
    };
    {
        let (proxy, state) = (proxy.clone(), state.clone());
        let spawned = std::thread::Builder::new()
            .name("beamer-hotkey-signals".into())
            .spawn(move || listen(proxy, state, tx));
        if let Err(e) = spawned {
            tracing::error!("Failed to spawn desktop hotkey listener: {}", e);
            return;
        }
    }

    let mut w = Worker { proxy, owned, wanted: Default::default(), pushed: None, warned: false };
    while let Ok(mut cmd) = rx.recv() {
        // Settings edits arrive in bursts; only the newest chords matter.
        while let Ok(next) = rx.try_recv() {
            if let (Cmd::Push(_), Cmd::Push(_)) = (&cmd, &next) {
                cmd = next;
            } else {
                w.handle(cmd, &state);
                cmd = next;
            }
        }
        w.handle(cmd, &state);
    }
}

struct Worker {
    proxy: zbus::blocking::Proxy<'static>,
    owned: DesktopOwned,
    /// The chords Beamer wants grabbed, re-pushed on `HelperEnabled`.
    wanted: Accels,
    /// What the extension currently holds; `None` = nothing, or unknown.
    pushed: Option<Accels>,
    /// Absent/old helper is logged once, not on every settings edit.
    warned: bool,
}

impl Worker {
    fn handle(&mut self, cmd: Cmd, state: &Mutex<HookState>) {
        match cmd {
            Cmd::Push(accels) => {
                self.wanted = accels;
                // `update_configs` fires on any config edit, most of which
                // don't touch the chords. Skip the ungrab/regrab churn.
                if self.pushed.as_ref() != Some(&self.wanted) {
                    self.push();
                }
            }
            Cmd::HelperEnabled => self.push(),
            Cmd::HelperDisabled => {
                self.pushed = None;
                release_desktop_bindings(&mut state.lock().unwrap_or_else(|p| p.into_inner()));
            }
        }
    }

    fn push(&mut self) {
        self.pushed = None;
        let grabbed = match self.set_hotkeys() {
            Ok(grabbed) => grabbed,
            Err(why) => {
                if !self.warned {
                    tracing::debug!("desktop hotkey: {}; evdev only", why);
                    self.warned = true;
                }
                vec![]
            }
        };
        for (i, owned) in self.owned.iter().enumerate() {
            owned.store(grabbed.get(i).copied().unwrap_or(false), Ordering::Relaxed);
        }
        if !grabbed.is_empty() {
            tracing::info!("Desktop hotkey grab: {:?} -> {:?}", self.wanted, grabbed);
            self.pushed = Some(self.wanted.clone());
            self.warned = false;
        }
    }

    fn set_hotkeys(&self) -> Result<Vec<bool>, String> {
        match self.proxy.call::<_, _, u32>("GetVersion", &()) {
            Ok(v) if v >= HOTKEY_VERSION => {}
            Ok(v) => return Err(format!("helper v{v} predates SetHotkeys (v{HOTKEY_VERSION})")),
            Err(e) => return Err(format!("helper not active ({e})")),
        }
        self.proxy
            .call::<_, _, Vec<bool>>("SetHotkeys", &(self.wanted.to_vec()))
            .map_err(|e| format!("SetHotkeys failed ({e})"))
    }
}

/// Route the extension's signals: hotkey edges straight into the shared state
/// machine, helper lifecycle to the worker.
fn listen(proxy: zbus::blocking::Proxy<'static>, state: Arc<Mutex<HookState>>, tx: Sender<Cmd>) {
    let signals = match proxy.receive_all_signals() {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("desktop hotkey: cannot subscribe to helper signals: {}", e);
            return;
        }
    };
    for msg in signals {
        let header = msg.header();
        let Some(member) = header.member() else { continue };
        let is_press = match member.as_str() {
            "HotkeyActivated" => true,
            "HotkeyDeactivated" => false,
            "HelperEnabled" => {
                let _ = tx.send(Cmd::HelperEnabled);
                continue;
            }
            "HelperDisabled" => {
                let _ = tx.send(Cmd::HelperDisabled);
                continue;
            }
            _ => continue,
        };
        let Ok(idx) = msg.body().deserialize::<u32>() else { continue };
        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
        on_grab_event(idx as usize, is_press, &mut state);
    }
}

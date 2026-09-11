use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use evdev::{Device, EventSummary, KeyCode};
use tokio::sync::mpsc::UnboundedSender;

use crate::hotkey::gnome_grab::{self, DesktopOwned, GrabHandle};
use crate::hotkey::{
    build_bindings, matching_binding, BindingConfig, BindingState, HotkeyConfig, HotkeyEvent,
    Modifiers, VK_LWIN, MAX_BINDINGS,
};

/// Rescan `/dev/input` this often so keyboards plugged in after startup
/// (or re-enumerated after suspend/Bluetooth reconnect) get listeners.
const DEVICE_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Press state shared by the evdev listeners and the GNOME desktop grab, so
/// both feed one state machine (`on_trigger`).
pub(super) struct HookState {
    bindings: Arc<Mutex<Vec<BindingConfig>>>,
    reset_flag: Arc<AtomicBool>,
    /// Per binding: the GNOME extension holds a live grab for it, so its
    /// presses come from `on_grab_event` and evdev's copies are ignored.
    /// Per-binding rather than global so a Super-only or refused chord
    /// stays on evdev.
    desktop_owned: DesktopOwned,
    tx: UnboundedSender<HotkeyEvent>,
    ctrl_held: bool,
    alt_held: bool,
    shift_held: bool,
    binding_state: [BindingState; MAX_BINDINGS],
}

impl HookState {
    pub(super) fn new(
        bindings: Arc<Mutex<Vec<BindingConfig>>>,
        reset_flag: Arc<AtomicBool>,
        desktop_owned: DesktopOwned,
        tx: UnboundedSender<HotkeyEvent>,
    ) -> Self {
        Self {
            bindings,
            reset_flag,
            desktop_owned,
            tx,
            ctrl_held: false,
            alt_held: false,
            shift_held: false,
            binding_state: [BindingState::default(); MAX_BINDINGS],
        }
    }

    fn is_desktop_owned(&self, idx: usize) -> bool {
        self.desktop_owned
            .get(idx)
            .is_some_and(|owned| owned.load(Ordering::Relaxed))
    }
}

/// Whether a device looks like it can produce hotkey chords. Deliberately
/// broad: macro pads, foot pedals and numpad-only devices fail a strict
/// A+Z+Space test and would otherwise be silently never listened to.
/// Non-keyboard devices are filtered by `supported_keys` being absent, and
/// every skip is logged so a missed device is diagnosable.
fn looks_like_keyboard(keys: &evdev::AttributeSetRef<KeyCode>) -> bool {
    keys.contains(KeyCode::KEY_A)
        || keys.contains(KeyCode::KEY_ENTER)
        || keys.contains(KeyCode::KEY_SPACE)
        || keys.contains(KeyCode::KEY_LEFTCTRL)
}

/// Find all keyboard devices in /dev/input/event*, skipping any whose
/// `/dev/input` path is already being watched.
fn find_keyboard_devices(known: &mut HashSet<PathBuf>) -> Vec<(PathBuf, Device)> {
    let mut keyboards = Vec::new();

    for (path, device) in evdev::enumerate() {
        if known.contains(&path) {
            continue;
        }
        match device.supported_keys() {
            Some(keys) if looks_like_keyboard(keys) => {
                tracing::info!(
                    "Found keyboard device: {:?} at {:?} ({:?})",
                    device.name(),
                    path,
                    device.physical_path()
                );
                known.insert(path.clone());
                keyboards.push((path, device));
            }
            _ => {
                tracing::debug!(
                    "Skipping non-keyboard input device: {:?} at {:?}",
                    device.name(),
                    path
                );
            }
        }
    }

    keyboards
}

/// Map evdev KeyCode to our portable VK code.
fn evdev_key_to_vk(key: KeyCode) -> Option<u32> {
    Some(match key {
        KeyCode::KEY_SPACE => 0x20,
        KeyCode::KEY_ENTER => 0x0D,
        KeyCode::KEY_TAB => 0x09,
        KeyCode::KEY_BACKSPACE => 0x08,
        KeyCode::KEY_DELETE => 0x2E,
        KeyCode::KEY_INSERT => 0x2D,
        KeyCode::KEY_HOME => 0x24,
        KeyCode::KEY_END => 0x23,
        KeyCode::KEY_PAGEUP => 0x21,
        KeyCode::KEY_PAGEDOWN => 0x22,
        KeyCode::KEY_UP => 0x26,
        KeyCode::KEY_DOWN => 0x28,
        KeyCode::KEY_LEFT => 0x25,
        KeyCode::KEY_RIGHT => 0x27,
        KeyCode::KEY_A => 0x41,
        KeyCode::KEY_B => 0x42,
        KeyCode::KEY_C => 0x43,
        KeyCode::KEY_D => 0x44,
        KeyCode::KEY_E => 0x45,
        KeyCode::KEY_F => 0x46,
        KeyCode::KEY_G => 0x47,
        KeyCode::KEY_H => 0x48,
        KeyCode::KEY_I => 0x49,
        KeyCode::KEY_J => 0x4A,
        KeyCode::KEY_K => 0x4B,
        KeyCode::KEY_L => 0x4C,
        KeyCode::KEY_M => 0x4D,
        KeyCode::KEY_N => 0x4E,
        KeyCode::KEY_O => 0x4F,
        KeyCode::KEY_P => 0x50,
        KeyCode::KEY_Q => 0x51,
        KeyCode::KEY_R => 0x52,
        KeyCode::KEY_S => 0x53,
        KeyCode::KEY_T => 0x54,
        KeyCode::KEY_U => 0x55,
        KeyCode::KEY_V => 0x56,
        KeyCode::KEY_W => 0x57,
        KeyCode::KEY_X => 0x58,
        KeyCode::KEY_Y => 0x59,
        KeyCode::KEY_Z => 0x5A,
        KeyCode::KEY_0 => 0x30,
        KeyCode::KEY_1 => 0x31,
        KeyCode::KEY_2 => 0x32,
        KeyCode::KEY_3 => 0x33,
        KeyCode::KEY_4 => 0x34,
        KeyCode::KEY_5 => 0x35,
        KeyCode::KEY_6 => 0x36,
        KeyCode::KEY_7 => 0x37,
        KeyCode::KEY_8 => 0x38,
        KeyCode::KEY_9 => 0x39,
        KeyCode::KEY_F1 => 0x70,
        KeyCode::KEY_F2 => 0x71,
        KeyCode::KEY_F3 => 0x72,
        KeyCode::KEY_F4 => 0x73,
        KeyCode::KEY_F5 => 0x74,
        KeyCode::KEY_F6 => 0x75,
        KeyCode::KEY_F7 => 0x76,
        KeyCode::KEY_F8 => 0x77,
        KeyCode::KEY_F9 => 0x78,
        KeyCode::KEY_F10 => 0x79,
        KeyCode::KEY_F11 => 0x7A,
        KeyCode::KEY_F12 => 0x7B,
        _ => return None,
    })
}

/// Apply a config edit, if one is pending. Both input paths call this before
/// acting, or an edit made while only the grab is firing (remoted in over
/// RDP, where evdev sees nothing) would never end the in-flight recording.
fn apply_pending_reset(state: &mut HookState) {
    if state.reset_flag.swap(false, Ordering::Relaxed) {
        // A config edit resets both bindings, not just one. Any in-flight
        // recording ends first (mirrors ll_hook.rs): without this a toggle
        // latched on before the edit desyncs.
        for bs in state.binding_state.iter_mut() {
            if bs.armed || bs.toggled_on {
                let _ = state.tx.send(HotkeyEvent::RecordStop);
            }
        }
        state.binding_state = [BindingState::default(); MAX_BINDINGS];
    }
}

pub(super) fn handle_key_event(key: KeyCode, value: i32, state: &mut HookState) {
    // evdev value: 0=release, 1=press, 2=repeat (ignored).
    let is_press = value == 1;
    let is_release = value == 0;
    if !is_press && !is_release {
        return;
    }

    apply_pending_reset(state);

    match key {
        KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => state.ctrl_held = is_press,
        KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => state.alt_held = is_press,
        KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => state.shift_held = is_press,
        _ => {}
    }

    let mods = Modifiers {
        ctrl: state.ctrl_held,
        alt: state.alt_held,
        shift: state.shift_held,
    };

    let bindings = state.bindings.lock().unwrap().clone();

    // Match Win by keycode: evdev reports left/right meta separately.
    let vk = if matches!(key, KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA) {
        Some(VK_LWIN)
    } else {
        evdev_key_to_vk(key)
    };
    let Some(vk) = vk else { return };

    // On release match the held binding, not the chord: modifiers may
    // already be up, which would find nothing and strand `armed`.
    let idx = if is_press {
        matching_binding(&bindings, vk, mods)
    } else {
        (0..bindings.len())
            .find(|&i| bindings[i].config.trigger_vk == vk && state.binding_state[i].trigger_held)
    };
    let Some(idx) = idx else { return };

    // Presses of a grabbed chord come from the grab alone. Taking evdev's copy
    // too would race it: a quick tap can land evdev's press *and* release
    // before the grab's D-Bus press arrives, and a toggle would flip twice.
    // Releases still pass: a stray one is a no-op (`trigger_held` is clear),
    // and it ends a local hold if the grab's release is ever lost.
    if is_press && state.is_desktop_owned(idx) {
        return;
    }
    on_trigger(idx, is_press, state);
}

/// Press/release from the GNOME desktop grab. Mirrors evdev's ownership rule
/// from the other side: presses count only while the grab owns the binding.
pub(super) fn on_grab_event(idx: usize, is_press: bool, state: &mut HookState) {
    apply_pending_reset(state);
    if is_press && !state.is_desktop_owned(idx) {
        return;
    }
    on_trigger(idx, is_press, state);
}

/// The desktop grab is gone (screen lock, extension disabled): hand every
/// binding it owned back to evdev. A hold it started ends here, because its
/// release would have come through the grab. A latched toggle stays latched,
/// so a screen lock mid-note doesn't cut the note off; the next press from
/// either path turns it off.
pub(super) fn release_desktop_bindings(state: &mut HookState) {
    for idx in 0..MAX_BINDINGS {
        if !state.desktop_owned[idx].swap(false, Ordering::Relaxed) {
            continue;
        }
        let bs = &mut state.binding_state[idx];
        bs.trigger_held = false;
        if bs.armed {
            bs.armed = false;
            tracing::info!("Hotkey triggered: RecordStop (desktop grab released)");
            let _ = state.tx.send(HotkeyEvent::RecordStop);
        }
    }
}

/// The press/release state machine both input paths share. `trigger_held`
/// makes a repeated press (or a second path's copy) a no-op.
pub(super) fn on_trigger(idx: usize, is_press: bool, state: &mut HookState) {
    let (mode, is_toggle) = {
        let bindings = state.bindings.lock().unwrap();
        let Some(binding) = bindings.get(idx) else { return };
        (binding.mode, binding.config.is_toggle)
    };
    let bs = &mut state.binding_state[idx];

    if is_press {
        if !bs.trigger_held {
            bs.trigger_held = true;
            if is_toggle {
                bs.toggled_on = !bs.toggled_on;
                let event = if bs.toggled_on {
                    tracing::info!("Hotkey triggered: RecordStart({:?}) (toggle)", mode);
                    HotkeyEvent::RecordStart(mode)
                } else {
                    tracing::info!("Hotkey triggered: RecordStop (toggle)");
                    HotkeyEvent::RecordStop
                };
                let _ = state.tx.send(event);
            } else {
                tracing::info!("Hotkey triggered: RecordStart({:?}) (hold)", mode);
                let _ = state.tx.send(HotkeyEvent::RecordStart(mode));
                bs.armed = true;
            }
        }
    } else if bs.trigger_held {
        bs.trigger_held = false;
        if bs.armed {
            tracing::info!("Hotkey triggered: RecordStop (hold release)");
            let _ = state.tx.send(HotkeyEvent::RecordStop);
            bs.armed = false;
        }
    }
}

/// Handle for updating the keyboard listener configuration from the main thread.
#[derive(Clone)]
pub struct HotkeyHandle {
    bindings: Arc<Mutex<Vec<BindingConfig>>>,
    reset_flag: Arc<AtomicBool>,
    grab: GrabHandle,
}

impl HotkeyHandle {
    pub fn update_configs(&self, inject: HotkeyConfig, note: Option<HotkeyConfig>) {
        let bindings = build_bindings(inject, note);
        let accels = gnome_grab::accelerators(&bindings);
        *self.bindings.lock().unwrap() = bindings;
        self.reset_flag.store(true, Ordering::Relaxed);
        self.grab.push(accels);
    }
}

/// Global keyboard listener on dedicated threads (one per device) via evdev.
/// Works on X11 and Wayland; requires the `input` group (or root).
pub fn start_ll_hook(
    inject: HotkeyConfig,
    note: Option<HotkeyConfig>,
    tx: UnboundedSender<HotkeyEvent>,
) -> HotkeyHandle {
    let initial = build_bindings(inject, note);
    let accels = gnome_grab::accelerators(&initial);
    let bindings = Arc::new(Mutex::new(initial));
    let reset_flag = Arc::new(AtomicBool::new(false));
    let desktop_owned = DesktopOwned::default();

    let state = Arc::new(Mutex::new(HookState::new(
        bindings.clone(),
        reset_flag.clone(),
        desktop_owned.clone(),
        tx,
    )));
    // Alongside evdev, not instead of it: RDP input never reaches /dev/input,
    // and evdev keeps any chord the desktop can't grab.
    let grab = gnome_grab::start(accels, state.clone(), desktop_owned);

    let mut known: HashSet<PathBuf> = HashSet::new();
    let keyboards = find_keyboard_devices(&mut known);
    if keyboards.is_empty() {
        tracing::error!(
            "No keyboard devices found! Make sure your user is in the 'input' group:\n\
             \n  sudo usermod -aG input $USER\n\
             \nThen log out and back in. Hotkeys will not work until this is fixed."
        );
        let _ = notify_rust::Notification::new()
            .appname("Beamer")
            .summary("Beamer — Hotkeys Disabled")
            .body("Run: sudo usermod -aG input $USER\nThen log out and back in.")
            .urgency(notify_rust::Urgency::Critical)
            .show();
    }

    tracing::info!("Monitoring {} keyboard device(s) via evdev", keyboards.len());

    // Listener exit drops the path from `known` so reconnects are re-attached.
    let known = Arc::new(Mutex::new(known));
    for (path, device) in keyboards {
        spawn_device_listener(path, device, state.clone(), known.clone());
    }

    {
        let known = known.clone();
        let state = state.clone();
        std::thread::Builder::new()
            .name("evdev-hotplug".into())
            .spawn(move || loop {
                std::thread::sleep(DEVICE_RESCAN_INTERVAL);
                let new_devices = {
                    let mut guard = known.lock().unwrap_or_else(|p| p.into_inner());
                    find_keyboard_devices(&mut guard)
                };
                for (path, device) in new_devices {
                    tracing::info!("New keyboard appeared at {:?} — attaching listener", path);
                    spawn_device_listener(path, device, state.clone(), known.clone());
                }
            })
            .expect("Failed to spawn keyboard hotplug watcher thread");
    }

    HotkeyHandle { bindings, reset_flag, grab }
}

/// Read one keyboard until it disappears; then drop its path from `known`.
fn spawn_device_listener(
    path: PathBuf,
    mut device: Device,
    state: Arc<Mutex<HookState>>,
    known: Arc<Mutex<HashSet<PathBuf>>>,
) {
    let name = device.name().unwrap_or("unknown").to_string();
    let thread_name = format!("evdev-kbd-{}", name);

    let spawned = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            tracing::debug!("Listening on keyboard: {}", name);
            loop {
                match device.fetch_events() {
                    Ok(events) => {
                        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
                        for event in events {
                            if let EventSummary::Key(_ev, key, value) = event.destructure() {
                                handle_key_event(key, value, &mut state);
                            }
                        }
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        } else {
                            // Unplugged or unreadable — this device is done.
                            tracing::info!("evdev listener for {} exiting: {}", name, e);
                            break;
                        }
                    }
                }
            }
            known
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&path);
        });

    if let Err(e) = spawned {
        tracing::error!("Failed to spawn keyboard listener thread: {}", e);
    }
}

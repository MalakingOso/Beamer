use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use evdev::{Device, EventSummary, KeyCode};
use tokio::sync::mpsc::UnboundedSender;

use crate::hotkey::{HotkeyConfig, HotkeyEvent, VK_LWIN};

/// How often to rescan `/dev/input` for keyboards that appeared after startup.
///
/// Devices used to be enumerated exactly once, so a keyboard plugged in later —
/// or one that re-enumerates after a suspend/resume or a Bluetooth reconnect —
/// never got a listener and the hotkey silently did nothing on it until Beamer
/// was restarted. Rescanning is a cheap directory walk plus an ioctl per node.
const DEVICE_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

struct HookState {
    config: Arc<Mutex<HotkeyConfig>>,
    reset_flag: Arc<AtomicBool>,
    tx: UnboundedSender<HotkeyEvent>,
    ctrl_held: bool,
    alt_held: bool,
    shift_held: bool,
    armed: bool,
    toggled_on: bool,
    trigger_held: bool,
}

/// Find all keyboard devices in /dev/input/event*, skipping any whose
/// `/dev/input` path is already being watched.
fn find_keyboard_devices(known: &mut HashSet<PathBuf>) -> Vec<(PathBuf, Device)> {
    let mut keyboards = Vec::new();

    for (path, device) in evdev::enumerate() {
        if known.contains(&path) {
            continue;
        }
        if let Some(keys) = device.supported_keys() {
            if keys.contains(KeyCode::KEY_A)
                && keys.contains(KeyCode::KEY_Z)
                && keys.contains(KeyCode::KEY_SPACE)
            {
                tracing::info!(
                    "Found keyboard device: {:?} at {:?} ({:?})",
                    device.name(),
                    path,
                    device.physical_path()
                );
                known.insert(path.clone());
                keyboards.push((path, device));
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

fn handle_key_event(key: KeyCode, value: i32, state: &mut HookState) {
    // value: 0=release, 1=press, 2=repeat (ignore repeat)
    let is_press = value == 1;
    let is_release = value == 0;
    if !is_press && !is_release {
        return;
    }

    if state.reset_flag.swap(false, Ordering::Relaxed) {
        state.armed = false;
        state.toggled_on = false;
        state.trigger_held = false;
    }

    // Update modifier tracking
    match key {
        KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => state.ctrl_held = is_press,
        KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => state.alt_held = is_press,
        KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => state.shift_held = is_press,
        _ => {}
    }

    let config = state.config.lock().unwrap();
    let trigger_vk = config.trigger_vk;
    let is_win_trigger = trigger_vk == VK_LWIN;
    let is_toggle = config.is_toggle;
    let req_ctrl = config.ctrl;
    let req_alt = config.alt;
    let req_shift = config.shift;
    drop(config);

    let is_trigger = if is_win_trigger {
        matches!(key, KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA)
    } else if let Some(vk) = evdev_key_to_vk(key) {
        vk == trigger_vk
    } else {
        false
    };

    if !is_trigger {
        return;
    }

    if is_press {
        if !state.trigger_held {
            let mods_match = state.ctrl_held == req_ctrl
                && state.alt_held == req_alt
                && state.shift_held == req_shift;

            if mods_match {
                state.trigger_held = true;
                if is_toggle {
                    state.toggled_on = !state.toggled_on;
                    let event = if state.toggled_on {
                        tracing::info!("Hotkey triggered: RecordStart (toggle)");
                        HotkeyEvent::RecordStart
                    } else {
                        tracing::info!("Hotkey triggered: RecordStop (toggle)");
                        HotkeyEvent::RecordStop
                    };
                    let _ = state.tx.send(event);
                } else {
                    tracing::info!("Hotkey triggered: RecordStart (hold)");
                    let _ = state.tx.send(HotkeyEvent::RecordStart);
                    state.armed = true;
                }
            }
        }
    } else if state.trigger_held {
        state.trigger_held = false;
        if state.armed {
            tracing::info!("Hotkey triggered: RecordStop (hold release)");
            let _ = state.tx.send(HotkeyEvent::RecordStop);
            state.armed = false;
        }
    }
}

/// Handle for updating the keyboard listener configuration from the main thread.
#[derive(Clone)]
pub struct HotkeyHandle {
    config: Arc<Mutex<HotkeyConfig>>,
    reset_flag: Arc<AtomicBool>,
}

impl HotkeyHandle {
    pub fn update_config(&self, new_config: HotkeyConfig) {
        *self.config.lock().unwrap() = new_config;
        self.reset_flag.store(true, Ordering::Relaxed);
    }
}

/// Start a global keyboard listener on dedicated threads (one per keyboard device).
/// Uses evdev to read directly from /dev/input — works on both X11 and Wayland.
/// Requires the user to be in the `input` group (or root).
pub fn start_ll_hook(
    initial_config: HotkeyConfig,
    tx: UnboundedSender<HotkeyEvent>,
) -> HotkeyHandle {
    let config = Arc::new(Mutex::new(initial_config));
    let reset_flag = Arc::new(AtomicBool::new(false));

    let state = Arc::new(Mutex::new(HookState {
        config: config.clone(),
        reset_flag: reset_flag.clone(),
        tx,
        ctrl_held: false,
        alt_held: false,
        shift_held: false,
        armed: false,
        toggled_on: false,
        trigger_held: false,
    }));

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

    // A device's path is dropped from `known` when its listener exits, so a
    // keyboard that disconnects and reconnects is picked up by the next rescan
    // rather than being remembered as "already watched" forever.
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

    HotkeyHandle { config, reset_flag }
}

/// Read events from one keyboard until it errors out or disappears, then drop
/// its path from `known` so a reconnect can be re-attached.
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
                            // Unplugged (ENODEV) or a genuine read error —
                            // either way this device is done.
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

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use crate::hotkey::HotkeyEvent;

/// Query the OS for whether a key is physically held right now.
/// This avoids stale state when key-up events are dropped by Windows.
fn is_key_physically_held(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) < 0 }
}

fn modifier_physically_held(left_vk: i32, right_vk: i32) -> bool {
    is_key_physically_held(left_vk) || is_key_physically_held(right_vk)
}

const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
pub const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

pub struct HotkeyConfig {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub trigger_vk: u32,
    pub is_toggle: bool,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            trigger_vk: 0x20, // VK_SPACE
            is_toggle: false,
        }
    }
}

impl HotkeyConfig {
    pub fn parse(hotkey_str: &str, is_toggle: bool) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut has_win = false;
        let mut key_str = String::new();

        for part in hotkey_str.split('+') {
            match part.trim().to_uppercase().as_str() {
                "CTRL" | "CONTROL" => ctrl = true,
                "ALT" | "OPTION" => alt = true,
                "SHIFT" => shift = true,
                "SUPER" | "WIN" | "CMD" | "COMMAND" | "META" => has_win = true,
                other => key_str = other.to_string(),
            }
        }

        let trigger_vk = if has_win && key_str.is_empty() {
            VK_LWIN
        } else if !key_str.is_empty() {
            key_name_to_vk(&key_str)?
        } else {
            0x20 // VK_SPACE default
        };

        Some(Self {
            ctrl,
            alt,
            shift,
            trigger_vk,
            is_toggle,
        })
    }
}

fn key_name_to_vk(name: &str) -> Option<u32> {
    match name.to_uppercase().as_str() {
        "SPACE" => Some(0x20),
        "ENTER" | "RETURN" => Some(0x0D),
        "TAB" => Some(0x09),
        "BACKSPACE" | "BACK" => Some(0x08),
        "DELETE" => Some(0x2E),
        "INSERT" => Some(0x2D),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" => Some(0x21),
        "PAGEDOWN" => Some(0x22),
        "UP" => Some(0x26),
        "DOWN" => Some(0x28),
        "LEFT" => Some(0x25),
        "RIGHT" => Some(0x27),
        s if s.len() == 1 => {
            let c = s.as_bytes()[0];
            if c.is_ascii_uppercase() || c.is_ascii_digit() {
                Some(c as u32)
            } else {
                None
            }
        }
        s if s.starts_with('F') && s.len() <= 3 => {
            let n: u32 = s[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                Some(0x70 + n - 1)
            } else {
                None
            }
        }
        _ => None,
    }
}

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
    win_consumed: bool,
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
    }

    let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode;
    let msg = wparam.0 as u32;
    let is_press = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let is_release = msg == WM_KEYUP || msg == WM_SYSKEYUP;

    if !is_press && !is_release {
        return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
    }

    let suppress = HOOK_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let state = match borrow.as_mut() {
            Some(s) => s,
            None => return false,
        };

        // Reset state if config was updated
        if state.reset_flag.swap(false, Ordering::Relaxed) {
            state.armed = false;
            state.toggled_on = false;
            state.trigger_held = false;
            state.win_consumed = false;
        }

        // Update modifier tracking
        match vk {
            VK_LCONTROL | VK_RCONTROL => state.ctrl_held = is_press,
            VK_LMENU | VK_RMENU => state.alt_held = is_press,
            VK_LSHIFT | VK_RSHIFT => state.shift_held = is_press,
            _ => {}
        }

        // Read config (brief lock)
        let config = state.config.lock().unwrap();
        let trigger_vk = config.trigger_vk;
        let is_win_trigger = trigger_vk == VK_LWIN;
        let is_toggle = config.is_toggle;
        let req_ctrl = config.ctrl;
        let req_alt = config.alt;
        let req_shift = config.shift;
        drop(config);

        // Check if this event is for the trigger key
        let is_trigger = if is_win_trigger {
            vk == VK_LWIN || vk == VK_RWIN
        } else {
            vk == trigger_vk
        };

        if !is_trigger {
            return false;
        }

        if is_press {
            if !state.trigger_held {
                // First press: check physical modifier state to avoid stale
                // tracked state (key-up events can be dropped by Windows)
                let ctrl_down = modifier_physically_held(
                    VK_LCONTROL as i32, VK_RCONTROL as i32,
                );
                let alt_down = modifier_physically_held(
                    VK_LMENU as i32, VK_RMENU as i32,
                );
                let shift_down = modifier_physically_held(
                    VK_LSHIFT as i32, VK_RSHIFT as i32,
                );
                let mods_match = ctrl_down == req_ctrl
                    && alt_down == req_alt
                    && shift_down == req_shift;
                // Sync tracked state to match reality
                state.ctrl_held = ctrl_down;
                state.alt_held = alt_down;
                state.shift_held = shift_down;

                if mods_match {
                    state.trigger_held = true;
                    if is_toggle {
                        state.toggled_on = !state.toggled_on;
                        let event = if state.toggled_on {
                            HotkeyEvent::RecordStart
                        } else {
                            HotkeyEvent::RecordStop
                        };
                        let _ = state.tx.send(event);
                    } else {
                        let _ = state.tx.send(HotkeyEvent::RecordStart);
                        state.armed = true;
                    }
                    if is_win_trigger {
                        state.win_consumed = true;
                    }
                }
            }
            // Suppress all Win key presses while consumed (including repeats)
            if is_win_trigger && state.win_consumed {
                return true;
            }
        } else {
            // Release
            if state.trigger_held {
                state.trigger_held = false;
                if state.armed {
                    let _ = state.tx.send(HotkeyEvent::RecordStop);
                    state.armed = false;
                }
            }
            if is_win_trigger && state.win_consumed {
                state.win_consumed = false;
                return true;
            }
        }

        false
    });

    if suppress {
        return LRESULT(1);
    }

    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

/// Handle for updating the LL keyboard hook configuration from the main thread.
#[derive(Clone)]
pub struct HotkeyHandle {
    config: Arc<Mutex<HotkeyConfig>>,
    reset_flag: Arc<AtomicBool>,
    thread_id: u32,
}

impl HotkeyHandle {
    pub fn update_config(&self, new_config: HotkeyConfig) {
        *self.config.lock().unwrap() = new_config;
        self.reset_flag.store(true, Ordering::Relaxed);
    }
}

impl Drop for HotkeyHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

/// Starts a low-level keyboard hook on a dedicated thread.
/// Returns a handle for updating the hotkey configuration.
pub fn start_ll_hook(
    initial_config: HotkeyConfig,
    tx: UnboundedSender<HotkeyEvent>,
) -> HotkeyHandle {
    let config = Arc::new(Mutex::new(initial_config));
    let reset_flag = Arc::new(AtomicBool::new(false));

    let config_clone = config.clone();
    let reset_clone = reset_flag.clone();
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("ll-keyboard-hook".into())
        .spawn(move || {
            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = Some(HookState {
                    config: config_clone,
                    reset_flag: reset_clone,
                    tx,
                    ctrl_held: false,
                    alt_held: false,
                    shift_held: false,
                    armed: false,
                    toggled_on: false,
                    trigger_held: false,
                    win_consumed: false,
                });
            });

            unsafe {
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0)
                    .expect("Failed to install keyboard hook");

                tid_tx.send(GetCurrentThreadId()).ok();

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}

                let _ = UnhookWindowsHookEx(hook);
            }

            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = None;
            });
        })
        .expect("Failed to spawn hook thread");

    let thread_id = tid_rx.recv().expect("Hook thread failed to start");

    HotkeyHandle {
        config,
        reset_flag,
        thread_id,
    }
}

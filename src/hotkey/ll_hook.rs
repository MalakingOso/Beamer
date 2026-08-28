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

use crate::hotkey::{
    build_bindings, matching_binding, BindingConfig, BindingState, HotkeyConfig, HotkeyEvent,
    Modifiers, VK_LWIN, MAX_BINDINGS,
};

const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_RWIN: u32 = 0x5C;

/// Query the OS for whether a key is physically held right now.
/// This avoids stale state when key-up events are dropped by Windows.
fn is_key_physically_held(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) < 0 }
}

fn modifier_physically_held(left_vk: i32, right_vk: i32) -> bool {
    is_key_physically_held(left_vk) || is_key_physically_held(right_vk)
}

// No tracked ctrl/alt/shift fields here (unlike linux_hotkey.rs's HookState):
// matching always uses a fresh `GetAsyncKeyState` read (see `hook_proc`
// below), so a tracked copy would be write-only dead state. Windows can drop
// key-up events for modifiers, which is exactly the staleness a tracked copy
// would reintroduce.
struct HookState {
    bindings: Arc<Mutex<Vec<BindingConfig>>>,
    reset_flag: Arc<AtomicBool>,
    tx: UnboundedSender<HotkeyEvent>,
    binding_state: [BindingState; MAX_BINDINGS],
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

        // Reset state if config was updated. Clears every binding, not just
        // one, since a config edit resets both.
        if state.reset_flag.swap(false, Ordering::Relaxed) {
            state.binding_state = [BindingState::default(); MAX_BINDINGS];
            state.win_consumed = false;
        }

        // Either side of Win triggers a VK_LWIN-configured binding, same as
        // linux_hotkey.rs normalises KEY_RIGHTMETA to VK_LWIN.
        let is_win_key_event = vk == VK_LWIN || vk == VK_RWIN;
        let norm_vk = if is_win_key_event { VK_LWIN } else { vk };

        let bindings = state.bindings.lock().unwrap();

        // Not a trigger key for any configured binding: nothing to do. This
        // also means modifier keys (Ctrl/Alt/Shift) never reach the work
        // below on their own, and are only queried live, on a trigger-key event,
        // via GetAsyncKeyState.
        if !bindings.iter().any(|b| b.config.trigger_vk == norm_vk) {
            return false;
        }

        if is_press {
            // A Windows LL hook gets no repeat bit in KBDLLHOOKSTRUCT (evdev
            // has one; linux_hotkey.rs drops repeats before this point), so
            // tracked held-state is the only way to tell an OS auto-repeat
            // WM_KEYDOWN apart from a fresh press. That test has to run
            // *before* matching, keyed by the physical trigger key rather
            // than by a specific binding: at most one binding can be
            // genuinely held on a given physical key at a time, so if any
            // binding sharing this trigger key is already marked held, this
            // event is a repeat of it, full stop. Matching (and therefore
            // resyncing modifiers) only runs on a real first press.
            //
            // Per-binding matching after that point would let a modifier
            // brushed mid-hold reroute a Win-key auto-repeat to a *different*
            // binding sharing the same trigger (e.g. tapping Alt while
            // holding Ctrl+Super mid-dictation would resync to Ctrl+Alt+Super
            // on the next repeat and fire the note binding without a fresh
            // press). Gating on the physical key first rules that out.
            //
            // Trade-off: if this trigger key's own key-up is ever dropped,
            // the stuck flag swallows exactly one subsequent press. It
            // self-heals on the next release, which always clears whichever
            // binding is still marked held (see below).
            let already_held = bindings
                .iter()
                .enumerate()
                .any(|(i, b)| b.config.trigger_vk == norm_vk && state.binding_state[i].trigger_held);

            if !already_held {
                // First press: check physical modifier state to avoid stale
                // tracked state (key-up events can be dropped by Windows).
                let ctrl_down = modifier_physically_held(VK_LCONTROL as i32, VK_RCONTROL as i32);
                let alt_down = modifier_physically_held(VK_LMENU as i32, VK_RMENU as i32);
                let shift_down = modifier_physically_held(VK_LSHIFT as i32, VK_RSHIFT as i32);
                let mods = Modifiers { ctrl: ctrl_down, alt: alt_down, shift: shift_down };

                if let Some(idx) = matching_binding(&bindings, norm_vk, mods) {
                    let binding = &bindings[idx];
                    let mode = binding.mode;
                    let is_toggle = binding.config.is_toggle;
                    let is_win_trigger = binding.config.trigger_vk == VK_LWIN;
                    let bs = &mut state.binding_state[idx];
                    bs.trigger_held = true;
                    if is_toggle {
                        bs.toggled_on = !bs.toggled_on;
                        let event = if bs.toggled_on {
                            HotkeyEvent::RecordStart(mode)
                        } else {
                            HotkeyEvent::RecordStop
                        };
                        let _ = state.tx.send(event);
                    } else {
                        let _ = state.tx.send(HotkeyEvent::RecordStart(mode));
                        bs.armed = true;
                    }
                    if is_win_trigger {
                        state.win_consumed = true;
                    }
                }
            }
            // Suppress all Win key presses while consumed (including repeats)
            if is_win_key_event && state.win_consumed {
                return true;
            }
        } else {
            // Release: the modifiers may already be up by the time the
            // trigger key is released, so re-matching the chord would find
            // nothing and strand the binding in `armed`. Find the binding
            // that is actually held instead.
            let idx = (0..bindings.len())
                .find(|&i| bindings[i].config.trigger_vk == norm_vk && state.binding_state[i].trigger_held);
            if let Some(idx) = idx {
                let bs = &mut state.binding_state[idx];
                bs.trigger_held = false;
                if bs.armed {
                    let _ = state.tx.send(HotkeyEvent::RecordStop);
                    bs.armed = false;
                }
            }
            if is_win_key_event && state.win_consumed {
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
    bindings: Arc<Mutex<Vec<BindingConfig>>>,
    reset_flag: Arc<AtomicBool>,
    thread_id: u32,
}

impl HotkeyHandle {
    pub fn update_configs(&self, inject: HotkeyConfig, note: Option<HotkeyConfig>) {
        *self.bindings.lock().unwrap() = build_bindings(inject, note);
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
    inject: HotkeyConfig,
    note: Option<HotkeyConfig>,
    tx: UnboundedSender<HotkeyEvent>,
) -> HotkeyHandle {
    let bindings = Arc::new(Mutex::new(build_bindings(inject, note)));
    let reset_flag = Arc::new(AtomicBool::new(false));

    let bindings_clone = bindings.clone();
    let reset_clone = reset_flag.clone();
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("ll-keyboard-hook".into())
        .spawn(move || {
            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = Some(HookState {
                    bindings: bindings_clone,
                    reset_flag: reset_clone,
                    tx,
                    binding_state: [BindingState::default(); MAX_BINDINGS],
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

    HotkeyHandle { bindings, reset_flag, thread_id }
}

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

/// Live OS query for whether a key is held (tracked state goes stale
/// when Windows drops key-up events).
fn is_key_physically_held(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) < 0 }
}

fn modifier_physically_held(left_vk: i32, right_vk: i32) -> bool {
    is_key_physically_held(left_vk) || is_key_physically_held(right_vk)
}

// No tracked modifier fields (unlike linux_hotkey.rs): matching always reads
// `GetAsyncKeyState` fresh, since Windows can drop modifier key-ups.
struct HookState {
    shared: Arc<Shared>,
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

        // A config edit resets both bindings. Any in-flight recording ends
        // first: without this a toggle latched on before the edit desyncs,
        // and the next toggle press sends RecordStart while already recording.
        if state.shared.reset_flag.swap(false, Ordering::Relaxed) {
            for bs in state.binding_state.iter_mut() {
                if bs.armed || bs.toggled_on {
                    let _ = state.tx.send(HotkeyEvent::RecordStop);
                }
            }
            state.binding_state = [BindingState::default(); MAX_BINDINGS];
            state.win_consumed = false;
        }

        // Either Win key triggers a VK_LWIN binding (mirrors linux_hotkey.rs).
        let is_win_key_event = vk == VK_LWIN || vk == VK_RWIN;
        let norm_vk = if is_win_key_event { VK_LWIN } else { vk };

        let bindings = state.shared.bindings.lock().unwrap_or_else(|p| p.into_inner());

        // Non-trigger keys (incl. bare modifiers) are only queried live via GetAsyncKeyState.
        if !bindings.iter().any(|b| b.config.trigger_vk == norm_vk) {
            return false;
        }

        if is_press {
            // No repeat bit in KBDLLHOOKSTRUCT, so held-state is the only
            // repeat detector. Gate on the physical key *before* matching so a
            // modifier brushed mid-hold can't reroute an auto-repeat to a
            // different binding sharing this trigger. Trade-off: a dropped
            // key-up swallows one press; the next release self-heals it.
            let already_held = bindings
                .iter()
                .enumerate()
                .any(|(i, b)| b.config.trigger_vk == norm_vk && state.binding_state[i].trigger_held);

            // A press the hook consumes must not also reach the focused app:
            // the default Ctrl+Space would otherwise type a space (or, e.g.,
            // clear Word's character formatting) on every press. Repeats count
            // as consumed while the trigger is still held from the first press.
            let mut consumed = already_held;

            if !already_held {
                // First press: read live modifier state (tracked state goes stale).
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
                    consumed = true;
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
            if consumed || (is_win_key_event && state.win_consumed) {
                return true;
            }
        } else {
            // On release match the held binding: modifiers may already be up,
            // so re-matching the chord would find nothing and strand `armed`.
            // A release ending a consumed hold is suppressed with its press.
            let mut consumed = false;
            let idx = (0..bindings.len())
                .find(|&i| bindings[i].config.trigger_vk == norm_vk && state.binding_state[i].trigger_held);
            if let Some(idx) = idx {
                let bs = &mut state.binding_state[idx];
                bs.trigger_held = false;
                consumed = true;
                if bs.armed {
                    let _ = state.tx.send(HotkeyEvent::RecordStop);
                    bs.armed = false;
                }
            }
            if consumed || (is_win_key_event && state.win_consumed) {
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

/// Shared hook-thread state. The thread stops only when the last handle
/// drops: posting `WM_QUIT` per clone would kill the hook out from under
/// surviving clones.
struct Shared {
    bindings: Mutex<Vec<BindingConfig>>,
    reset_flag: AtomicBool,
    /// Filled in by the hook thread once its message loop exists; read by
    /// `Drop` to address the quit message.
    thread_id: std::sync::atomic::AtomicU32,
}

/// Handle for updating the LL keyboard hook configuration from the main thread.
#[derive(Clone)]
pub struct HotkeyHandle {
    shared: Arc<Shared>,
}

impl HotkeyHandle {
    pub fn update_configs(&self, inject: HotkeyConfig, note: Option<HotkeyConfig>) {
        *self.shared.bindings.lock().unwrap_or_else(|p| p.into_inner()) =
            build_bindings(inject, note);
        self.shared.reset_flag.store(true, Ordering::Relaxed);
    }
}

impl Drop for HotkeyHandle {
    fn drop(&mut self) {
        // `self` still counts: 1 means this is the last owner.
        if Arc::strong_count(&self.shared) == 1 {
            let thread_id = self.shared.thread_id.load(Ordering::Relaxed);
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
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
    let shared = Arc::new(Shared {
        bindings: Mutex::new(build_bindings(inject, note)),
        reset_flag: AtomicBool::new(false),
        thread_id: std::sync::atomic::AtomicU32::new(0),
    });

    let thread_shared = shared.clone();
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("ll-keyboard-hook".into())
        .spawn(move || {
            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = Some(HookState {
                    shared: thread_shared.clone(),
                    tx,
                    binding_state: [BindingState::default(); MAX_BINDINGS],
                    win_consumed: false,
                });
            });

            unsafe {
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0)
                    .expect("Failed to install keyboard hook");

                thread_shared.thread_id.store(GetCurrentThreadId(), Ordering::Relaxed);
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
    shared.thread_id.store(thread_id, Ordering::Relaxed);

    HotkeyHandle { shared }
}

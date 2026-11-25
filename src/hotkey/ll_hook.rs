//! Windows hotkey listener: a `WH_KEYBOARD_LL` hook that turns the configured
//! chords into `HotkeyEvent`s and swallows them so the focused app never sees them.
//!
//! The one exception to the spawn_blocking rule for Win32: the hook must own a
//! dedicated thread running a `GetMessageW` loop, never a runtime pool thread.
//! The hook proc runs on that thread, hence the `thread_local!` state.
//! Unelevated, it never sees keys bound for elevated windows (UIPI).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KillTimer, PostThreadMessageW, SetTimer, SetWindowsHookExW,
    UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
    WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
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

/// Periodic idle hook refresh (ms): Windows can silently remove `WH_KEYBOARD_LL`
/// if CPU contention trips `LowLevelHooksTimeout`; reinstalling while idle
/// brings hotkeys back without restarting Beamer.
const HOOK_REFRESH_MS: u32 = 60_000;

fn trigger_physically_held(trigger_vk: u32) -> bool {
    if trigger_vk == VK_LWIN {
        is_key_physically_held(VK_LWIN as i32) || is_key_physically_held(VK_RWIN as i32)
    } else {
        is_key_physically_held(trigger_vk as i32)
    }
}

// No tracked modifier fields (unlike linux_hotkey.rs): matching always reads
// `GetAsyncKeyState` fresh, since Windows can drop modifier key-ups.
struct HookState {
    shared: Arc<Shared>,
    /// Thread-local mirror of `shared.bindings`, refreshed only when
    /// `reset_flag` fires — keeps `hook_proc` completely lock-free on the hot
    /// path so contention can never stall Windows key delivery.
    bindings: Vec<BindingConfig>,
    tx: UnboundedSender<HotkeyEvent>,
    binding_state: [BindingState; MAX_BINDINGS],
    win_consumed: bool,
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

/// Self-heal any held binding whose physical key is already released (dropped
/// key-up from UAC, Win+L lock screen, Ctrl+Alt+Del, or an elevated window).
fn reconcile_stale_holds(state: &mut HookState) {
    let mut any_win_held = false;
    for (i, b) in state.bindings.iter().enumerate() {
        let bs = &mut state.binding_state[i];
        if bs.trigger_held {
            if !trigger_physically_held(b.config.trigger_vk) {
                bs.trigger_held = false;
                if bs.armed {
                    bs.armed = false;
                    let _ = state.tx.send(HotkeyEvent::RecordStop);
                }
            } else if b.config.trigger_vk == VK_LWIN {
                any_win_held = true;
            }
        }
    }
    if state.win_consumed && !any_win_held && !trigger_physically_held(VK_LWIN) {
        state.win_consumed = false;
    }
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

        // A config edit resets all bindings and refreshes the thread-local copy.
        if state.shared.reset_flag.swap(false, Ordering::Acquire) {
            for bs in state.binding_state.iter_mut() {
                if bs.armed || bs.toggled_on {
                    let _ = state.tx.send(HotkeyEvent::RecordStop);
                }
            }
            state.binding_state = [BindingState::default(); MAX_BINDINGS];
            state.win_consumed = false;
            if let Ok(guard) = state.shared.bindings.try_lock() {
                state.bindings = guard.clone();
            } else {
                // Re-arm flag so the next key event picks up the update without blocking here.
                state.shared.reset_flag.store(true, Ordering::Release);
            }
        }

        // Either Win key triggers a VK_LWIN binding (mirrors linux_hotkey.rs).
        let is_win_key_event = vk == VK_LWIN || vk == VK_RWIN;
        let norm_vk = if is_win_key_event { VK_LWIN } else { vk };

        // Non-trigger keys (incl. bare modifiers): self-heal any dropped key-up, then pass through.
        if !state.bindings.iter().any(|b| b.config.trigger_vk == norm_vk) {
            reconcile_stale_holds(state);
            return false;
        }

        if is_press {
            // KBDLLHOOKSTRUCT has no repeat bit, so held-state detects repeats.
            // Check it *before* matching so a modifier brushed mid-hold can't
            // reroute a repeat to another binding sharing this trigger.
            let already_held = state
                .bindings
                .iter()
                .enumerate()
                .any(|(i, b)| b.config.trigger_vk == norm_vk && state.binding_state[i].trigger_held);

            let mut consumed = already_held;

            if !already_held {
                let ctrl_down = modifier_physically_held(VK_LCONTROL as i32, VK_RCONTROL as i32);
                let alt_down = modifier_physically_held(VK_LMENU as i32, VK_RMENU as i32);
                let shift_down = modifier_physically_held(VK_LSHIFT as i32, VK_RSHIFT as i32);
                let mods = Modifiers { ctrl: ctrl_down, alt: alt_down, shift: shift_down };

                if let Some(idx) = matching_binding(&state.bindings, norm_vk, mods) {
                    let binding = &state.bindings[idx];
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
            let mut consumed = false;
            let idx = (0..state.bindings.len())
                .find(|&i| state.bindings[i].config.trigger_vk == norm_vk && state.binding_state[i].trigger_held);
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
    pub fn update_configs(
        &self,
        inject: HotkeyConfig,
        note: Option<HotkeyConfig>,
        done: Option<HotkeyConfig>,
    ) {
        *self.shared.bindings.lock().unwrap_or_else(|p| p.into_inner()) =
            build_bindings(inject, note, done);
        self.shared.reset_flag.store(true, Ordering::Release);
    }
}

impl Drop for HotkeyHandle {
    fn drop(&mut self) {
        // `self` still counts: 1 means this is the last owner.
        // Never pass thread_id == 0: PostThreadMessageW(0, ...) targets the calling thread.
        if Arc::strong_count(&self.shared) == 1 {
            let thread_id = self.shared.thread_id.load(Ordering::Acquire);
            if thread_id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
}

/// Starts a low-level keyboard hook on a dedicated thread.
/// Returns a handle for updating the hotkey configuration.
pub fn start_ll_hook(
    inject: HotkeyConfig,
    note: Option<HotkeyConfig>,
    done: Option<HotkeyConfig>,
    tx: UnboundedSender<HotkeyEvent>,
) -> HotkeyHandle {
    let initial_bindings = build_bindings(inject, note, done);
    let shared = Arc::new(Shared {
        bindings: Mutex::new(initial_bindings.clone()),
        reset_flag: AtomicBool::new(false),
        thread_id: std::sync::atomic::AtomicU32::new(0),
    });

    let thread_shared = shared.clone();
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();

    let spawned = std::thread::Builder::new()
        .name("ll-keyboard-hook".into())
        .spawn(move || {
            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = Some(HookState {
                    shared: thread_shared.clone(),
                    bindings: initial_bindings,
                    tx,
                    binding_state: [BindingState::default(); MAX_BINDINGS],
                    win_consumed: false,
                });
            });

            unsafe {
                let mut hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
                    Ok(hook) => hook,
                    Err(e) => {
                        tracing::error!("Failed to install the keyboard hook: {e}");
                        return;
                    }
                };

                let timer_id = SetTimer(None, 0, HOOK_REFRESH_MS, None);
                thread_shared.thread_id.store(GetCurrentThreadId(), Ordering::Release);
                tid_tx.send(GetCurrentThreadId()).ok();

                let mut msg = MSG::default();
                loop {
                    let ret = GetMessageW(&mut msg, None, 0, 0).0;
                    if ret <= 0 {
                        if ret < 0 {
                            tracing::error!("GetMessageW in keyboard hook returned -1; exiting loop");
                        }
                        break;
                    }
                    if msg.message == WM_TIMER {
                        let idle = HOOK_STATE.with(|cell| {
                            let mut borrow = cell.borrow_mut();
                            let Some(state) = borrow.as_mut() else { return false };
                            reconcile_stale_holds(state);
                            !state.win_consumed
                                && state
                                    .binding_state
                                    .iter()
                                    .all(|bs| !bs.trigger_held && !bs.armed && !bs.toggled_on)
                        });
                        if idle {
                            if let Ok(new_hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
                                let _ = UnhookWindowsHookEx(hook);
                                hook = new_hook;
                            }
                        }
                    }
                }

                if timer_id != 0 {
                    let _ = KillTimer(None, timer_id);
                }
                let _ = UnhookWindowsHookEx(hook);
            }

            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = None;
            });
        });

    let started = match spawned {
        Ok(_) => tid_rx.recv().ok(),
        Err(e) => {
            tracing::error!("Failed to spawn the keyboard hook thread: {e}");
            None
        }
    };
    match started {
        Some(thread_id) => shared.thread_id.store(thread_id, Ordering::Release),
        None => crate::orchestrator::notify::show_notification(
            "Beamer",
            "Hotkeys are unavailable: the keyboard hook could not start. Restart Beamer.",
        ),
    }

    HotkeyHandle { shared }
}

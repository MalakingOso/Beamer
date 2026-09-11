//! Recording-pill window hook for `App`. Runs once from `App()`'s render body.
//! Linux uses an AppIndicator tray-icon swap instead (see `linux_integration.rs`).

#[cfg(not(target_os = "linux"))]
use dioxus::desktop::tao::dpi::{PhysicalPosition, PhysicalSize};
#[cfg(target_os = "windows")]
use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
#[cfg(not(target_os = "linux"))]
use dioxus::desktop::{Config as DesktopConfig, DesktopContext, WindowBuilder};
#[cfg(not(target_os = "linux"))]
use dioxus::prelude::*;

#[cfg(not(target_os = "linux"))]
use crate::config::Config;
#[cfg(not(target_os = "linux"))]
use crate::hotkey::CaptureMode;
#[cfg(not(target_os = "linux"))]
use crate::orchestrator::RecordingState;
#[cfg(not(target_os = "linux"))]
use crate::ui::pill::{
    RecordingPill, PILL_CSS, PILL_INK_BOTTOM, PILL_JS, PILL_WINDOW_H, PILL_WINDOW_W,
};

// Recording pill: small, transparent, click-through, always-on-top. Linux uses
// an AppIndicator tray-icon swap instead (see `linux_integration.rs`).
#[cfg(not(target_os = "linux"))]
pub(super) fn setup_recording_pill(
    window: DesktopContext,
    rec_state: Signal<RecordingState>,
    active_mode: Signal<CaptureMode>,
    config: Signal<Config>,
) {
    let mut pill_ctx: Signal<Option<DesktopContext>> = use_signal(|| None);
    let mut pill_click_through_set: Signal<bool> = use_signal(|| false);
    // Edge-detects hidden->visible: reposition only when the pill was not
    // already up, not on every recording -> processing change.
    let mut pill_was_shown: Signal<bool> = use_signal(|| false);
    let mut pill_size: Signal<(u32, u32)> = use_signal(|| (0, 0));

    use_hook({
        let window = window.clone();
        move || {
            spawn(async move {
                let scale = window
                    .primary_monitor()
                    .map(|m| m.scale_factor())
                    .unwrap_or(1.0);
                let monitor_size = window
                    .primary_monitor()
                    .map(|m| m.size())
                    .unwrap_or(PhysicalSize::new(1920, 1080));

                // Window deliberately larger than the resting pill: the border,
                // shadow and entrance-animation offset paint outside it and the
                // webview clips at its viewport edge.
                let pill_w = (PILL_WINDOW_W * scale) as u32;
                let pill_h = (PILL_WINDOW_H * scale) as u32;
                pill_size.set((pill_w, pill_h));
                let x = (monitor_size.width.saturating_sub(pill_w)) / 2;
                // Pill *ink* 60px above the bottom edge. Windows repositions on
                // first show (`reposition_to_foreground_monitor`); macOS keeps this.
                let y = monitor_size
                    .height
                    .saturating_sub(((PILL_INK_BOTTOM + 60.0) * scale) as u32);

                #[allow(unused_mut)]
                let mut builder = WindowBuilder::new()
                    .with_title("Beamer Recording")
                    .with_decorations(false)
                    .with_transparent(true)
                    .with_always_on_top(true)
                    .with_visible(false)
                    .with_focusable(false)
                    .with_inner_size(PhysicalSize::new(pill_w, pill_h))
                    .with_position(PhysicalPosition::new(x as i32, y as i32));
                #[cfg(target_os = "windows")]
                let builder = builder
                    .with_skip_taskbar(true)
                    .with_undecorated_shadow(false);

                let cfg = DesktopConfig::new()
                    .with_data_directory(super::webview_data_dir())
                    .with_window(builder)
                    .with_background_color((0, 0, 0, 0))
                    // Bundled DM Mono (no Google Fonts request; renders offline).
                    // `.pill` starts hidden in CSS; `beamerSetState` animates it.
                    .with_custom_head(format!(
                        r#"<style>{}{}</style><script>{}</script>"#,
                        crate::assets::dm_mono_face_css(),
                        PILL_CSS,
                        PILL_JS
                    ))
                    .with_exits_when_last_window_closes(false);

                let dom = VirtualDom::new(RecordingPill);
                let ctx: DesktopContext = window.new_window(dom, cfg).await;

                // Realize immediately so click-through applies, then hide: nothing
                // shows until recording starts. No DWM backdrop (Acrylic falls
                // back to opaque over RDP); plain transparent CSS is known-good.
                #[cfg(target_os = "windows")]
                {
                    ctx.set_visible(true);
                    let _ = ctx.set_ignore_cursor_events(true);
                    pill_click_through_set.set(true);
                    ctx.set_visible(false);
                }

                pill_ctx.set(Some(ctx));
            });
        }
    });

    use_effect(move || {
        let state = *rec_state.read();
        let mode = *active_mode.read();
        let pill_enabled = config.read().appearance.pill_enabled;
        if let Some(ctx) = pill_ctx.read().as_ref() {
            let js_state = crate::ui::pill::pill_state(state, mode).filter(|_| pill_enabled);
            if let Some(js_state) = js_state {
                // Windows set click-through at creation; macOS does it lazily below.
                ctx.set_visible(true);
                #[cfg(not(target_os = "windows"))]
                {
                    if !*pill_click_through_set.read() {
                        let _ = ctx.set_ignore_cursor_events(true);
                        pill_click_through_set.set(true);
                    }
                }

                // Follow the foreground monitor only on hidden->visible (see above).
                if !*pill_was_shown.read() {
                    pill_was_shown.set(true);
                    #[cfg(target_os = "windows")]
                    reposition_to_foreground_monitor(ctx, *pill_size.read());
                }

                let _ = ctx
                    .webview
                    .evaluate_script(&format!("beamerSetState('{js_state}');"));
            } else {
                pill_was_shown.set(false);
                let _ = ctx.webview.evaluate_script("beamerSetState('idle');");
                // 200ms matches beamerSetState's exit animation; hide as the fade ends.
                let ctx_clone = ctx.clone();
                spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    ctx_clone.set_visible(false);
                });
            }
        }
    });

    // Mic levels into the pill waveform at ~15Hz (mirrors linux_integration).
    use_hook(move || {
        spawn(async move {
            let mut level_rx = crate::audio::subscribe_levels();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(66)).await;
                if level_rx.has_changed().is_err() {
                    break;
                }
                let level = *level_rx.borrow_and_update();
                if *rec_state.peek() == RecordingState::Recording {
                    if let Some(ctx) = pill_ctx.peek().as_ref() {
                        let _ = ctx.webview.evaluate_script(&format!("beamerSetLevel({level});"));
                    }
                }
            }
        });
    });
}

/// Move the pill to the bottom-center of the foreground window's monitor
/// (`GetForegroundWindow` + `MonitorFromWindow`, matched back to tao by
/// `HMONITOR` handle as in `work_area.rs`). A failed lookup keeps the last
/// position — stale beats hidden.
#[cfg(target_os = "windows")]
fn reposition_to_foreground_monitor(ctx: &DesktopContext, (pill_w, pill_h): (u32, u32)) {
    use dioxus::desktop::tao::platform::windows::MonitorHandleExtWindows;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    if pill_w == 0 || pill_h == 0 {
        return;
    }

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return;
    }
    let target = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };

    let Some(monitor) = ctx
        .available_monitors()
        .find(|m| HMONITOR(m.hmonitor() as *mut std::ffi::c_void) == target)
    else {
        return;
    };

    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let hm = HMONITOR(monitor.hmonitor() as *mut std::ffi::c_void);
    if !unsafe { GetMonitorInfoW(hm, &mut info) }.as_bool() {
        return;
    }
    // `rcWork`, not `rcMonitor`: an always-on-top window renders *behind* the
    // taskbar, and `rcWork` already excludes it (as in `work_area.rs`).
    let rc = info.rcWork;
    let scale = monitor.scale_factor();
    // 32px below the pill's *ink*; the window's transparent animation headroom
    // already counts towards that gap, so it comes back out of the margin.
    let margin = ((32.0 - (PILL_WINDOW_H - PILL_INK_BOTTOM)) * scale) as i32;

    let work_w = (rc.right - rc.left).max(0);
    let work_h = (rc.bottom - rc.top).max(0);
    let x = rc.left + (work_w - pill_w as i32) / 2;
    let y = rc.top + work_h - pill_h as i32 - margin;
    ctx.set_outer_position(PhysicalPosition::new(x, y));
}

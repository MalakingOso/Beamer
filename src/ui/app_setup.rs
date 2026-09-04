//! Window/splash/tray/menu wiring hooks for `App`. Each runs once from within
//! `App()`'s render body, so hooks still run in a fixed order every render.

use dioxus::desktop::tao::dpi::{PhysicalPosition, PhysicalSize};
#[cfg(target_os = "windows")]
use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
use dioxus::desktop::trayicon::{init_tray_icon, MouseButton, MouseButtonState, TrayIconEvent};
use dioxus::desktop::{use_muda_event_handler, use_tray_icon_event_handler};
use dioxus::desktop::{Config as DesktopConfig, DesktopContext, WindowBuilder};
use dioxus::prelude::*;

use crate::config::Config;
#[cfg(not(target_os = "linux"))]
use crate::hotkey::CaptureMode;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
#[cfg(not(target_os = "linux"))]
use crate::orchestrator::RecordingState;
use crate::tray::{self, TrayMenuItems};
use crate::ui::status_log::{log_status, LogLevel, StatusLog};
use crate::update::{self, UpdateStatus};
#[cfg(not(target_os = "linux"))]
use crate::ui::pill::{
    RecordingPill, PILL_CSS, PILL_INK_BOTTOM, PILL_JS, PILL_WINDOW_H, PILL_WINDOW_W,
};
use crate::ui::splash::{SplashWindow, SPLASH_CSS};
use crate::warmup::{self, WarmupProgress};

use super::app::Page;

/// Build the tray menu and register the tray icon.
pub(super) fn setup_tray_menu() -> TrayMenuItems {
    use_hook(|| {
        let (menu, items) = tray::build_tray_menu();
        let icon = tray::load_icon();
        init_tray_icon(menu, Some(icon));
        items
    })
}

pub(super) fn setup_window_centering(window: DesktopContext) {
    use_hook({
        let window = window.clone();
        move || {
            if let Some(monitor) = window.primary_monitor() {
                let monitor_size = monitor.size();
                let scale = monitor.scale_factor();
                let win_w = (500.0 * scale) as i32;
                let win_h = (600.0 * scale) as i32;
                let x = (monitor_size.width as i32 - win_w) / 2;
                let y = (monitor_size.height as i32 - win_h) / 2;
                window.set_outer_position(PhysicalPosition::new(x, y));
            }
        }
    });
}

/// Cold-start warmup splash: pays the one-time costs that would otherwise stall
/// the first recording, then closes itself and flips `app_ready`.
pub(super) fn setup_splash(window: DesktopContext, mut app_ready: Signal<bool>) {
    let warmup_progress = use_signal(WarmupProgress::default);
    let mut splash_ctx: Signal<Option<DesktopContext>> = use_signal(|| None);

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

                let splash_w = (280.0 * scale) as u32;
                let splash_h = (280.0 * scale) as u32;
                let x = (monitor_size.width.saturating_sub(splash_w)) / 2;
                let y = (monitor_size.height.saturating_sub(splash_h)) / 2;

                #[allow(unused_mut)]
                let mut builder = WindowBuilder::new()
                    .with_title("Beamer")
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_transparent(true)
                    .with_always_on_top(true)
                    .with_focusable(false)
                    .with_inner_size(PhysicalSize::new(splash_w, splash_h))
                    .with_position(PhysicalPosition::new(x as i32, y as i32));
                #[cfg(target_os = "windows")]
                let builder = builder
                    .with_skip_taskbar(true)
                    .with_undecorated_shadow(false);

                let cfg = DesktopConfig::new()
                    .with_data_directory(super::webview_data_dir())
                    .with_window(builder)
                    .with_background_color((0, 0, 0, 0))
                    .with_custom_head(format!("<style>{}</style>", SPLASH_CSS))
                    .with_exits_when_last_window_closes(false);

                let dom = VirtualDom::new(SplashWindow);
                let ctx: DesktopContext = window.new_window(dom, cfg).await;
                splash_ctx.set(Some(ctx));

                let started = std::time::Instant::now();
                warmup::warm_all(warmup_progress).await;

                // Floor matches the splash bar's 1500ms CSS keyframe (see SPLASH_CSS).
                let min_visible = std::time::Duration::from_millis(1500);
                let elapsed = started.elapsed();
                if elapsed < min_visible {
                    tokio::time::sleep(min_visible - elapsed).await;
                }

                if let Some(ctx) = splash_ctx.read().as_ref() {
                    ctx.close();
                }
                splash_ctx.set(None);

                app_ready.set(true);

                // Non-Windows reveals the main window here; Windows waits for tray-click.
                #[cfg(not(target_os = "windows"))]
                window.set_visible(true);
            });
        }
    });
}

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

/// Coalesce per-keystroke edits into at most one write per tick.
const NOTES_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Debounced writes for both stores. `peek()`, not `read()`: a `write()` every
/// tick would notify every subscriber twice a second. Task done-ticks ride
/// here too (accept/dismiss decisions flush inline instead).
pub(super) fn setup_notes_flush(mut notes: Signal<NoteStore>, mut tasks: Signal<TaskStore>) {
    use_hook(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(NOTES_FLUSH_INTERVAL).await;
                // `needs_flush`, not `is_dirty`: an inline flush clears `dirty`
                // when the JSON mirror lands while the document still owes a
                // write. Third arm is the mtime stat catching a synced-in document.
                let pending = notes.peek().needs_flush()
                    || tasks.peek().needs_flush()
                    || notes.peek().doc_file_moved();
                if pending {
                    let mut notes = notes.write();
                    let mut tasks = tasks.write();
                    crate::notes::flush_stores(&mut notes, &mut tasks);
                }
            }
        });
    });
}

/// Surface corpus load failures in the status log: a quarantined store recovers
/// silently otherwise, and the user must know their notes did not come back.
pub(super) fn report_load_errors(
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    mut status_log: Signal<StatusLog>,
) {
    use_hook(move || {
        for message in [notes.peek().load_error.clone(), tasks.peek().load_error.clone()]
            .into_iter()
            .flatten()
        {
            log_status(&mut status_log, LogLevel::Error, message);
        }
    });
}

/// Background update check on startup (3s delay to keep launch snappy).
pub(super) fn setup_update_check(config: Signal<Config>, mut update_status: Signal<UpdateStatus>) {
    use_hook({
        let auto_check = config.peek().appearance.auto_check_updates;
        move || {
            if auto_check {
                spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    update_status.set(UpdateStatus::Checking);
                    let result = tokio::task::spawn_blocking(update::check_for_update_blocking).await;
                    match result {
                        Ok(Ok(Some(info))) => {
                            tracing::info!("Update available: v{}", info.version);
                            update_status.set(UpdateStatus::Available { version: info.version });
                        }
                        Ok(Ok(None)) => {
                            tracing::debug!("No update available");
                            update_status.set(UpdateStatus::Idle);
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("Update check failed: {}", e);
                            update_status.set(UpdateStatus::Idle);
                        }
                        Err(e) => {
                            tracing::warn!("Update check task panicked: {}", e);
                            update_status.set(UpdateStatus::Idle);
                        }
                    }
                });
            }
        }
    });
}

/// Register Beamer's AppUserModelID via its Start Menu shortcut (COM + fs work,
/// so off the render thread). Best-effort: a stale shortcut only degrades toast
/// branding, never blocks dictation.
#[cfg(target_os = "windows")]
pub(super) fn setup_windows_aumid_shortcut() {
    use_hook(|| {
        spawn(async move {
            if let Err(e) = tokio::task::spawn_blocking(super::windows_shortcut::ensure_shortcut).await {
                tracing::warn!("AUMID shortcut task panicked: {}", e);
            }
        });
    });
}

/// Tray menu clicks. `use_muda_event_handler`, not the tray variant: a
/// dioxus-desktop 0.7.3 bug delivers tray clicks as MudaMenuEvent.
pub(super) fn setup_menu_handlers(
    items: &TrayMenuItems,
    window: DesktopContext,
    mut current_page: Signal<Page>,
    last_injection: Signal<String>,
    config: Signal<Config>,
    mut update_status: Signal<UpdateStatus>,
    mut notes: Signal<NoteStore>,
    mut tasks: Signal<TaskStore>,
) {
    use_muda_event_handler({
        let home_id = items.home.id().clone();
        let history_id = items.history.id().clone();
        let vocab_id = items.vocab.id().clone();
        let settings_id = items.settings.id().clone();
        let paste_last_id = items.paste_last.id().clone();
        let check_updates_id = items.check_updates.id().clone();
        let quit_id = items.quit.id().clone();
        let window = window.clone();
        move |event| {
            if event.id == quit_id {
                tracing::info!("Quit menu item clicked — exiting");
                // `process::exit` skips the flush tick and destructors, so flush
                // here and release the single-instance guard explicitly.
                crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
                crate::release_single_instance();
                std::process::exit(0);
            } else if event.id == home_id {
                current_page.set(Page::Home);
                window.set_visible(true);
                window.set_focus();
            } else if event.id == history_id {
                current_page.set(Page::History);
                window.set_visible(true);
                window.set_focus();
            } else if event.id == vocab_id {
                current_page.set(Page::Vocab);
                window.set_visible(true);
                window.set_focus();
            } else if event.id == settings_id {
                current_page.set(Page::Settings);
                window.set_visible(true);
                window.set_focus();
            } else if event.id == paste_last_id {
                let text = last_injection.read().clone();
                if text != "No injection yet" && !text.is_empty() {
                    let backends = config.read().injection.backends.clone();
                    let paste_shortcut = config.read().injection.paste_shortcut.clone();
                    spawn(async move {
                        if let Err(e) = crate::injection::inject_text(&text, &backends, &paste_shortcut).await {
                            tracing::error!("Paste last transcript failed: {e}");
                        }
                    });
                }
            } else if event.id == check_updates_id {
                spawn(async move {
                    update_status.set(UpdateStatus::Checking);
                    let result = tokio::task::spawn_blocking(update::check_for_update_blocking).await;
                    match result {
                        Ok(Ok(Some(info))) => {
                            update_status.set(UpdateStatus::Available { version: info.version });
                        }
                        Ok(Ok(None)) => {
                            update_status.set(UpdateStatus::Idle);
                        }
                        Ok(Err(e)) => {
                            update_status.set(UpdateStatus::Error(e.to_string()));
                        }
                        Err(e) => {
                            update_status.set(UpdateStatus::Error(format!("Task panicked: {e}")));
                        }
                    }
                });
                current_page.set(Page::Settings);
                window.set_visible(true);
                window.set_focus();
            }
        }
    });
}

/// Left-click the tray icon toggles the main window.
pub(super) fn setup_tray_click_handler(window: DesktopContext) {
    use_tray_icon_event_handler({
        let window = window.clone();
        move |event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if window.is_visible() {
                    window.set_visible(false);
                } else {
                    window.set_visible(true);
                    window.set_focus();
                }
            }
        }
    });
}

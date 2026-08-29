//! Window/splash/tray/menu wiring hooks for `App`.
//!
//! These are Dioxus hooks extracted verbatim out of `app.rs` into standalone
//! functions — each is called once from within `App()`'s render body, so the
//! usual "hooks must run in a fixed order every render" rule still holds.

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
use crate::ui::pill::{RecordingPill, PILL_CSS, PILL_JS};
use crate::ui::splash::{SplashWindow, SPLASH_CSS};
use crate::warmup::{self, WarmupProgress};

use super::app::Page;

/// Build the tray menu and register the tray icon. Runs once on first render.
pub(super) fn setup_tray_menu() -> TrayMenuItems {
    use_hook(|| {
        let (menu, items) = tray::build_tray_menu();
        let icon = tray::load_icon();
        init_tray_icon(menu, Some(icon));
        items
    })
}

/// Center the window on the primary monitor (runs once on first render).
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

/// Cold-start warmup splash. Runs once on first render: opens a small
/// centered window, walks `warm_all` through keyring/audio/(mpris)/network,
/// then closes itself. Pays the one-time costs that would otherwise stall
/// the first recording.
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

                // The splash bar fills via a CSS keyframe over 1500ms (see
                // SPLASH_CSS), so the floor must match that duration — otherwise
                // the splash dismisses while the fill is still animating.
                let min_visible = std::time::Duration::from_millis(1500);
                let elapsed = started.elapsed();
                if elapsed < min_visible {
                    tokio::time::sleep(min_visible - elapsed).await;
                }

                if let Some(ctx) = splash_ctx.read().as_ref() {
                    ctx.close();
                }
                splash_ctx.set(None);

                // Sticky notes restored from disk wait on this — the loading
                // screen going away is the signal, not a fixed delay.
                app_ready.set(true);

                // Reveal the main window on platforms where it would have
                // shown at launch. Windows keeps it hidden until tray-click,
                // matching the original tray-app convention.
                #[cfg(not(target_os = "windows"))]
                window.set_visible(true);
            });
        }
    });
}

// Recording pill window — small, transparent, click-through, always-on-top.
// Linux: the pill is replaced by an AppIndicator tray-icon swap (see
// `linux_integration.rs`). GNOME Shell doesn't accept in-tray GTK widgets
// from standalone apps, and the floating-pill approach has
// compositor/transparency quirks under Wayland.
#[cfg(not(target_os = "linux"))]
pub(super) fn setup_recording_pill(
    window: DesktopContext,
    rec_state: Signal<RecordingState>,
    active_mode: Signal<CaptureMode>,
    config: Signal<Config>,
) {
    let mut pill_ctx: Signal<Option<DesktopContext>> = use_signal(|| None);
    let mut pill_click_through_set: Signal<bool> = use_signal(|| false);
    // Edge-detects the hidden->visible transition, same as indicator.js's
    // `wasVisible` check in `show()`: reposition and play the entrance
    // animation only when the pill was not already up, not on every
    // recording -> processing state change while it stays on screen.
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

                let pill_w = (220.0 * scale) as u32;
                let pill_h = (52.0 * scale) as u32;
                pill_size.set((pill_w, pill_h));
                let x = (monitor_size.width.saturating_sub(pill_w)) / 2;
                let y = monitor_size.height.saturating_sub(pill_h + (60.0 * scale) as u32);

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
                    // DM Mono is inlined from the bundled woff2 rather than
                    // fetched from fonts.googleapis.com: no outbound request
                    // from a local dictation app, and the pill renders in the
                    // right typeface offline. No body-level opacity wrapper
                    // here any more — `.pill` in PILL_CSS starts hidden
                    // (opacity:0, scaled/translated down) on its own, and
                    // `beamerSetState` animates it in/out, so nothing extra
                    // is needed to hide it before the first state arrives.
                    .with_custom_head(format!(
                        r#"<style>{}{}</style><script>{}</script>"#,
                        crate::assets::dm_mono_face_css(),
                        PILL_CSS,
                        PILL_JS
                    ))
                    .with_exits_when_last_window_closes(false);

                let dom = VirtualDom::new(RecordingPill);
                let ctx: DesktopContext = window.new_window(dom, cfg).await;

                // On Windows, realize the window immediately so
                // set_ignore_cursor_events works, then hide it again. Nothing
                // should show until recording actually starts. No DWM system
                // backdrop here — see the removed `apply_windows_pill_backdrop`
                // in git history: Acrylic silently falls back to a solid
                // color over Remote Desktop (the compositor can't blend with
                // desktop content), and that fallback painted an opaque box
                // behind the pill's own CSS, which had already been thinned
                // out on the assumption the material was actually showing.
                // The plain transparent CSS capsule below is the known-good
                // state confirmed on bearcave before Acrylic was ever added.
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

    // Update pill appearance when recording state changes (Windows/macOS only).
    use_effect(move || {
        let state = *rec_state.read();
        let mode = *active_mode.read();
        let pill_enabled = config.read().appearance.pill_enabled;
        if let Some(ctx) = pill_ctx.read().as_ref() {
            let js_state = crate::ui::pill::pill_state(state, mode).filter(|_| pill_enabled);
            if let Some(js_state) = js_state {
                // Windows already realized the window and set click-through at
                // creation (see the constructor above), so it only needs
                // showing here. macOS does both lazily, on first show.
                ctx.set_visible(true);
                #[cfg(not(target_os = "windows"))]
                {
                    if !*pill_click_through_set.read() {
                        let _ = ctx.set_ignore_cursor_events(true);
                        pill_click_through_set.set(true);
                    }
                }

                // Same edge as indicator.js's `wasVisible`: follow the
                // foreground window's monitor only on the hidden->visible
                // transition, not on every recording -> processing update.
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
                // Hide on every platform this function runs on (Windows and
                // macOS): CSS opacity alone used to be Windows's only defence
                // against a visible idle window, and that defence only works
                // when WebView2 transparency actually takes. 200ms matches
                // beamerSetState's own exit-animation duration, so the window
                // disappears right as the fade-out finishes.
                let ctx_clone = ctx.clone();
                spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    ctx_clone.set_visible(false);
                });
            }
        }
    });

    // Pump live mic levels into the pill's waveform (~15Hz) — the Windows/
    // macOS twin of linux_integration.rs's identically-commented loop, so
    // both platforms' bars react to the same signal at the same rate.
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

/// Move the pill window to the bottom-center of whatever monitor the
/// foreground window is on, mirroring indicator.js's `_reposition()`
/// (`BOTTOM_MARGIN = 32`, run once per show rather than continuously). tao
/// has no cross-platform "which monitor is window X on" query, so this goes
/// straight to Win32: `GetForegroundWindow` + `MonitorFromWindow`, then
/// matched back to a tao `MonitorHandle` by comparing `HMONITOR` handles —
/// the same match-by-handle approach `work_area.rs`'s `windows_work_rect`
/// uses for the taskbar-aware rectangle.
///
/// A failed lookup (no foreground window, or its monitor not found among
/// tao's enumerated ones) leaves the pill at its last position rather than
/// erroring — a stale position is a much smaller mistake than a hidden pill.
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
    let rc = info.rcMonitor;
    let scale = monitor.scale_factor();
    let margin = (32.0 * scale) as i32;

    let mon_w = (rc.right - rc.left).max(0);
    let mon_h = (rc.bottom - rc.top).max(0);
    let x = rc.left + (mon_w - pill_w as i32) / 2;
    let y = rc.top + mon_h - pill_h as i32 - margin;
    ctx.set_outer_position(PhysicalPosition::new(x, y));
}

/// How often the notes store is checked for pending edits.
///
/// Note bodies are edited per keystroke; writing the whole file on each one
/// would be pathological, so edits coalesce into at most one write per tick.
const NOTES_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Drive the debounced writes for both note stores.
///
/// Dirtiness is checked through `peek()` rather than `read()` on purpose: a
/// `write()` on every tick would notify every subscriber — including each open
/// sticky window — twice a second, whether or not anything had changed.
///
/// Tasks ride the same tick. Their two *decisions* flush inline, since a lost
/// decision is lost eval signal — but ticking a task done does not, and without
/// this that checkbox would live in memory until some later accept or dismiss
/// happened to write the file.
pub(super) fn setup_notes_flush(mut notes: Signal<NoteStore>, mut tasks: Signal<TaskStore>) {
    use_hook(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(NOTES_FLUSH_INTERVAL).await;
                // `needs_flush` rather than `is_dirty`: an inline flush
                // clears `dirty` as soon as the JSON mirror lands, and the
                // automerge document still owes a write at that point. The
                // third arm is the mtime stat that catches a document synced
                // in from the other machine.
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

/// Push whatever went wrong while loading the corpus into the status log.
///
/// A corrupt `notes.automerge` is quarantined and replaced by an empty store,
/// which is the right recovery and the wrong silence: the user has to be told
/// their notes did not come back. Runs once, on the first render.
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

/// Register Beamer's AppUserModelID by creating (or repairing) its Start
/// Menu shortcut. Runs once on first render, off the render thread: shortcut
/// creation is COM and filesystem work, so it goes through
/// `tokio::task::spawn_blocking` the same way `injection::inject_text` does
/// for UIA, rather than blocking the Dioxus event loop.
///
/// Best-effort. See `ui::windows_shortcut::ensure_shortcut` for the failure
/// handling. A missing or stale shortcut only degrades toast branding; it
/// never blocks dictation.
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

/// Wire up tray menu item clicks.
///
/// NOTE: use_muda_event_handler instead of use_tray_menu_event_handler because
/// dioxus-desktop 0.7.3 has a bug: set_menubar_receiver() claims the muda OnceCell
/// before set_tray_icon_receiver(), so tray menu clicks arrive as MudaMenuEvent,
/// never as TrayMenuEvent.
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
                // `process::exit` below skips the 500ms flush tick along with
                // every destructor, so any note edit still sitting in memory
                // has to be written out here or it dies with the process.
                crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
                // `process::exit` skips destructors, so the single-instance
                // guard has to be handed back explicitly or the lockfile
                // outlives us.
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

/// Left-click the tray icon to toggle the main window's visibility.
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

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
use crate::orchestrator::RecordingState;
use crate::tray::{self, TrayMenuItems};
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
pub(super) fn setup_splash(window: DesktopContext) {
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
                let builder = builder.with_skip_taskbar(true);

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
    config: Signal<Config>,
) {
    let mut pill_ctx: Signal<Option<DesktopContext>> = use_signal(|| None);
    let mut pill_click_through_set: Signal<bool> = use_signal(|| false);

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
                let builder = builder.with_skip_taskbar(true);

                let cfg = DesktopConfig::new()
                    .with_data_directory(super::webview_data_dir())
                    .with_window(builder)
                    .with_background_color((0, 0, 0, 0))
                    // DM Mono is inlined from the bundled woff2 rather than
                    // fetched from fonts.googleapis.com: no outbound request
                    // from a local dictation app, and the pill renders in the
                    // right typeface offline.
                    .with_custom_head(format!(
                        r#"<style>{}body{{opacity:0;transition:opacity 0.15s ease;}}{}</style><script>{}</script>"#,
                        crate::assets::dm_mono_face_css(),
                        PILL_CSS,
                        PILL_JS
                    ))
                    .with_exits_when_last_window_closes(false);

                let dom = VirtualDom::new(RecordingPill);
                let ctx: DesktopContext = window.new_window(dom, cfg).await;

                // On Windows, realize the window immediately so
                // set_ignore_cursor_events works.
                #[cfg(target_os = "windows")]
                {
                    ctx.set_visible(true);
                    let _ = ctx.set_ignore_cursor_events(true);
                    pill_click_through_set.set(true);
                }

                pill_ctx.set(Some(ctx));
            });
        }
    });

    // Update pill appearance when recording state changes (Windows/macOS only).
    use_effect(move || {
        let state = *rec_state.read();
        let pill_enabled = config.read().appearance.pill_enabled;
        if let Some(ctx) = pill_ctx.read().as_ref() {
            let should_show = state != RecordingState::Idle && pill_enabled;
            if should_show {
                let js_state = match state {
                    RecordingState::Recording => "recording",
                    RecordingState::Processing => "processing",
                    _ => unreachable!(),
                };

                // On macOS, realize the window and set click-through on first show.
                #[cfg(not(target_os = "windows"))]
                {
                    ctx.set_visible(true);
                    if !*pill_click_through_set.read() {
                        let _ = ctx.set_ignore_cursor_events(true);
                        pill_click_through_set.set(true);
                    }
                }

                let _ = ctx
                    .webview
                    .evaluate_script(&format!("beamerSetState('{js_state}');"));
            } else {
                let _ = ctx.webview.evaluate_script("beamerSetState('idle');");
                #[cfg(not(target_os = "windows"))]
                {
                    let ctx_clone = ctx.clone();
                    spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        ctx_clone.set_visible(false);
                    });
                }
            }
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

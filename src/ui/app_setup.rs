//! Window, tray-menu and flush wiring hooks for `App`. Each runs once from within
//! `App()`'s render body, so hooks still run in a fixed order every render.
//! Splash, recording pill and menu handlers live in their own modules
//! (`app_splash`, `app_pill`, `app_menu`).

use dioxus::desktop::tao::dpi::PhysicalPosition;
use dioxus::desktop::trayicon::init_tray_icon;
use dioxus::desktop::DesktopContext;
use dioxus::prelude::*;

use crate::config::Config;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::tray::{self, TrayMenuItems};
use crate::ui::status_log::{log_status, LogLevel, StatusLog};
use crate::update::{self, UpdateStatus};

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

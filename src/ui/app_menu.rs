//! Tray menu and tray-click event wiring for `App`. Runs once from `App()`'s render body.

use dioxus::desktop::trayicon::{MouseButton, MouseButtonState, TrayIconEvent};
use dioxus::desktop::{use_muda_event_handler, use_tray_icon_event_handler};
use dioxus::desktop::DesktopContext;
use dioxus::prelude::*;

use crate::config::Config;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::tray::TrayMenuItems;
use crate::update::{self, UpdateStatus};
use super::app::Page;

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

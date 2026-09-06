#![cfg(target_os = "linux")]

//! Linux: tray icon follows recording state; on GNOME the helper extension
//! additionally shows a shell-native recording pill (a Wayland window can't be
//! positioned or kept always-on-top, so the pill lives inside Shell).

use dioxus::prelude::*;

use crate::config::Config;
use crate::hotkey::CaptureMode;
use crate::orchestrator::RecordingState;
use crate::tray;

use crate::ui::pill::pill_state;

pub(super) fn setup_linux_integration(
    rec_state: Signal<RecordingState>,
    active_mode: Signal<CaptureMode>,
    config: Signal<Config>,
) {
    use dioxus::desktop::trayicon::use_tray_icon;
    let tray_handle = use_tray_icon();
    use_effect(move || {
        let state = *rec_state.read();
        if let Some(tray) = tray_handle.as_ref() {
            let icon = match state {
                RecordingState::Idle => tray::load_icon(),
                RecordingState::Recording | RecordingState::Processing => {
                    tray::load_recording_icon()
                }
            };
            let _ = tray.set_icon(Some(icon));
        }

        // Read, not peek: a mode change — or toggling the pill itself —
        // must re-run this effect.
        let mode = *active_mode.read();
        let pill_enabled = config.read().appearance.pill_enabled;
        match pill_state(state, mode).filter(|_| pill_enabled) {
            Some(style) => crate::ui::shell_indicator::show(style),
            None => crate::ui::shell_indicator::hide(),
        }
    });

    // Pump mic levels into the shell pill at ~15Hz (no-op without the helper).
    // Fixed 66ms tick rather than waking on the 100+Hz audio callback; the
    // `has_changed` break is defensive — the sender lives for the process.
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
                    crate::ui::shell_indicator::update_level(level);
                }
            }
        });
    });
}

#![cfg(target_os = "linux")]

//! Linux: swap the tray icon to reflect recording state (mirrors Handy's
//! behavior). GNOME's AppIndicator extension renders the tray icon in the
//! top bar; tray-icon wraps libappindicator and `set_icon` writes a PNG to
//! /tmp and signals a reload. On GNOME the helper extension additionally
//! shows a shell-native recording pill (waveform + timer) — a regular
//! Wayland window can't be positioned or kept always-on-top, so the pill
//! lives inside GNOME Shell instead.

use dioxus::prelude::*;

use crate::config::Config;
use crate::orchestrator::RecordingState;
use crate::tray;

pub(super) fn setup_linux_integration(rec_state: Signal<RecordingState>, config: Signal<Config>) {
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

        let pill_enabled = config.peek().appearance.pill_enabled;
        match state {
            RecordingState::Recording if pill_enabled => {
                crate::ui::shell_indicator::show("recording")
            }
            RecordingState::Processing if pill_enabled => {
                crate::ui::shell_indicator::show("processing")
            }
            _ => crate::ui::shell_indicator::hide(),
        }
    });

    // Pump live mic levels into the shell pill's waveform (~15 Hz).
    // The worker thread no-ops when the helper extension isn't active.
    //
    // Interval-driven rather than `changed().await`-driven: the audio
    // callback publishes levels at 100+ Hz, and waking on every update
    // just to throttle back down to 15 Hz wastes wakeups. Polling on a
    // fixed 66ms tick and reading whatever the watch channel currently
    // holds (`borrow_and_update`) decouples our wakeup rate from the
    // audio callback rate while keeping the same visible cadence.
    // `has_changed()` still detects sender drop (audio stream torn
    // down) so this loop doesn't spin forever afterward.
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

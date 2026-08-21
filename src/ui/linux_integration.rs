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
use crate::hotkey::CaptureMode;
use crate::orchestrator::RecordingState;
use crate::tray;

/// Which shell-pill style a recording state and capture mode select, or `None`
/// to hide the pill.
///
/// Factored out of the effect below purely so it can be tested: the effect
/// itself needs a live Dioxus runtime and a D-Bus connection, while this — the
/// part that can actually be wrong — needs neither.
///
/// The state names are a contract with `indicator.js`; unknown values there
/// fall through to a labelled idle sweep rather than erroring, so a typo here
/// would look like a working pill that simply ignores your microphone.
pub(super) fn pill_state(state: RecordingState, mode: CaptureMode) -> Option<&'static str> {
    match (state, mode) {
        (RecordingState::Idle, _) => None,
        (RecordingState::Recording, CaptureMode::Note) => Some("note"),
        (RecordingState::Recording, CaptureMode::Inject) => Some("recording"),
        // Transcribing looks the same either way. The destination is already
        // decided by this point and the pill's only job is to say "working".
        (RecordingState::Processing, _) => Some("processing"),
    }
}

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

        // Read, not peek: the mode is part of what this effect renders, so a
        // mode change has to re-run it.
        let mode = *active_mode.read();
        let pill_enabled = config.peek().appearance.pill_enabled;
        match pill_state(state, mode).filter(|_| pill_enabled) {
            Some(style) => crate::ui::shell_indicator::show(style),
            None => crate::ui::shell_indicator::hide(),
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
    //
    // The `level_rx.has_changed().is_err()` break below is belt-and-braces
    // only: the `watch::Sender` this subscribes to lives in a `static
    // OnceLock` (see `level_channel()` in `src/audio/mod.rs`) and is never
    // dropped for the life of the process, so this branch is unreachable in
    // practice — the loop actually runs for as long as this Dioxus coroutine
    // does. Kept as a defensive exit in case that invariant ever changes.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_capture_gets_its_own_pill_style() {
        assert_eq!(
            pill_state(RecordingState::Recording, CaptureMode::Note),
            Some("note"),
            "dictating into a note instead of the focused field must never be a surprise"
        );
        assert_eq!(
            pill_state(RecordingState::Recording, CaptureMode::Inject),
            Some("recording")
        );
    }

    #[test]
    fn transcribing_looks_the_same_whatever_the_destination() {
        assert_eq!(
            pill_state(RecordingState::Processing, CaptureMode::Note),
            Some("processing")
        );
        assert_eq!(
            pill_state(RecordingState::Processing, CaptureMode::Inject),
            Some("processing")
        );
    }

    #[test]
    fn idle_hides_the_pill_in_either_mode() {
        assert_eq!(pill_state(RecordingState::Idle, CaptureMode::Note), None);
        assert_eq!(pill_state(RecordingState::Idle, CaptureMode::Inject), None);
    }

    #[test]
    fn every_style_is_one_the_extension_knows() {
        // indicator.js branches on these exact strings and silently treats an
        // unknown one as "not recording" — a labelled idle sweep that ignores
        // the microphone. That failure has no error and no log line.
        for state in [RecordingState::Idle, RecordingState::Recording, RecordingState::Processing] {
            for mode in [CaptureMode::Inject, CaptureMode::Note] {
                if let Some(style) = pill_state(state, mode) {
                    assert!(
                        matches!(style, "recording" | "processing" | "note"),
                        "{style:?} is not a state indicator.js handles"
                    );
                }
            }
        }
    }
}

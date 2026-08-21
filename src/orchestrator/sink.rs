//! Terminal sinks for a finished transcript.
//!
//! Split out of `mod.rs` to keep that file under the 500-line limit. The audio,
//! VAD, backend and tail-capture paths are shared by both capture modes; only
//! what happens to the final text differs.

use dioxus::prelude::*;

use crate::config::Config;
use crate::hotkey::CaptureMode;
use crate::notes::{NoteColor, NoteStore};
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Whether a transcript is worth persisting. A recording that produced only
/// silence should not leave an empty sticky note on the desktop.
pub(super) fn should_create_note(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Whether this mode's transcript goes to the injection chain.
pub(super) fn sink_injects(mode: CaptureMode) -> bool {
    matches!(mode, CaptureMode::Inject)
}

/// Create a sticky note from a finished transcript.
///
/// Returns the new note's id so the caller can open its window, or `None` when
/// there was nothing worth keeping.
///
/// The store is flushed to disk immediately rather than left to the ~500ms
/// debounce tick. The debounce exists for per-keystroke body edits, which are
/// cheap to lose and instantly retypeable; a just-captured transcript is
/// neither, and the spec's hard constraint is that a note must never be lost
/// because something downstream failed. This mirrors `TranscriptionHistory`,
/// which also writes inline from the orchestrator coroutine.
pub(super) async fn do_note_capture(
    text: &str,
    notes: &mut Signal<NoteStore>,
    config: &Signal<Config>,
    status_log: &mut Signal<StatusLog>,
) -> Option<String> {
    if !should_create_note(text) {
        log_status(status_log, LogLevel::Info, "Nothing captured — no note created");
        return None;
    }

    let color = NoteColor::from_config_name(&config.peek().notes.default_color);
    let id = {
        let mut store = notes.write();
        let id = store.create(text.to_string(), color);
        store.flush_if_dirty();
        id
    };

    log_status(
        status_log,
        LogLevel::Info,
        format!("Note created ({} chars)", text.trim().len()),
    );
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_transcripts_do_not_create_notes() {
        assert!(!should_create_note(""));
        assert!(!should_create_note("   "));
        assert!(!should_create_note("\n\t "));
        assert!(
            should_create_note("call the vet"),
            "real speech must always produce a note"
        );
    }

    #[test]
    fn note_mode_routes_away_from_injection() {
        assert!(!sink_injects(CaptureMode::Note));
        assert!(sink_injects(CaptureMode::Inject));
    }
}

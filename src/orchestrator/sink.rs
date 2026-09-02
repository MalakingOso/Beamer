//! Terminal sinks for a finished transcript.
//!
//! Split out of `mod.rs` to keep that file under the 500-line limit. The audio,
//! VAD, backend and tail-capture paths are shared by both capture modes; only
//! what happens to the final text differs — which is the whole of this module.
//!
//! `deliver` is the single place the inject-vs-note decision is made. All three
//! `TranscriptKind::Final` sites in `mod.rs` funnel through it rather than
//! repeating the branch.

use dioxus::prelude::*;

use super::notify::{clipboard_only_fallback, show_notification};
use crate::config::Config;
use crate::hotkey::CaptureMode;
use crate::injection;
use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::{NoteColor, NoteOrigin, NoteStore};
use crate::ui::history::TranscriptionHistory;
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
/// The corpus is flushed to disk immediately rather than left to the ~500ms
/// debounce tick. The debounce exists for per-keystroke body edits, which are
/// cheap to lose and instantly retypeable; a just-captured transcript is
/// neither, and the spec's hard constraint is that a note must never be lost
/// because something downstream failed. This mirrors `TranscriptionHistory`,
/// which also writes inline from the orchestrator coroutine.
///
/// ⚠️ It has to be `flush_stores`, not `NoteStore::flush_if_dirty`. That one
/// writes `notes.json`, which is a derived export nothing reads back once
/// `notes.automerge` exists, so a crash inside the tick would lose a note
/// whose audio is already gone. `flush_stores` is the single document writer,
/// so calling it here adds a call site and not a second writer, which is why
/// `tasks` is threaded down to this function at all.
pub(super) async fn do_note_capture(
    text: &str,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    _config: &Signal<Config>,
    status_log: &mut Signal<StatusLog>,
    note_passes: Coroutine<PipelineRequest>,
) -> Option<String> {
    if !should_create_note(text) {
        log_status(status_log, LogLevel::Info, "Nothing captured — no note created");
        return None;
    }

    let id = notes
        .write()
        .create(text.to_string(), NoteColor::random(), NoteOrigin::Dictated);
    crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());

    log_status(
        status_log,
        LogLevel::Info,
        format!("Note created ({} chars)", text.trim().len()),
    );

    // Ask for the model passes **after** the flush above. The note is on disk
    // before anything else is attempted, so no failure downstream — server
    // down, model asleep, GPU busy — can cost the user words.
    //
    // This is also the *only* site that triggers a pass automatically, and it
    // is reachable only from dictation. The rule "typed notes are never
    // rewritten unasked" is therefore structural rather than a runtime check
    // somebody could forget: a typed note has no path to this line.
    note_passes.send(PipelineRequest::for_new_note(&id));

    Some(id)
}

/// Route one finished transcript to the sink its capture mode selects.
///
/// Every path that produced a final transcript funnels through here, so the
/// inject-vs-note decision is made in exactly one place rather than repeated at
/// each of the three `TranscriptKind::Final` sites.
#[allow(clippy::too_many_arguments)]
pub(super) async fn deliver(
    text: &str,
    capture_mode: CaptureMode,
    backends: &[String],
    paste_shortcut: &str,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    status_log: &mut Signal<StatusLog>,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    config: &Signal<Config>,
    note_passes: Coroutine<PipelineRequest>,
) {
    if sink_injects(capture_mode) {
        do_injection(text, backends, paste_shortcut, last_injection, history, status_log).await;
    } else if let Some(id) =
        do_note_capture(text, notes, tasks, config, status_log, note_passes).await
    {
        // Nothing here opens or places a window. The reconciler in
        // `ui::sticky_windows` watches the store and does both.
        tracing::info!("note {} created", id);
    }
}

/// Inject transcribed text into the focused window using the configured fallback chain.
async fn do_injection(
    text: &str,
    backends: &[String],
    paste_shortcut: &str,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    status_log: &mut Signal<StatusLog>,
) {
    match injection::inject_text(text, backends, paste_shortcut).await {
        Ok(result) => {
            let status = format!("{}: {}", result.method, result.target_info);
            tracing::info!("Injected via {}", status);
            log_status(status_log, LogLevel::Info, format!("Injected via {}", status));
            last_injection.set(status);
        }
        Err(e) => {
            tracing::error!("Injection failed: {}, trying clipboard-only fallback", e);
            // Last resort: copy to clipboard and notify user to paste manually
            match clipboard_only_fallback(text).await {
                Ok(()) => {
                    let msg = "Copied to clipboard — press Ctrl+V to paste";
                    tracing::info!("{}", msg);
                    log_status(status_log, LogLevel::Info, msg.to_string());
                    last_injection.set(msg.to_string());
                    show_notification("Beamer", msg);
                }
                Err(cb_err) => {
                    tracing::error!("Clipboard fallback also failed: {}", cb_err);
                    log_status(status_log, LogLevel::Error, format!("Injection failed: {}", e));
                    last_injection.set(format!("Failed: {}", e));
                }
            }
        }
    }

    history.write().append(text.to_string());
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

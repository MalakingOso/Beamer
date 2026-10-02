//! Pure tests for `pipeline.rs` (split out for the 500-line limit). No
//! coroutine, signal or server: CI has no llama.cpp server, so the sweep is
//! tested through `sweep_requests` (what it asks for) and `should_sweep`
//! (whether it fires).

use super::*;
use crate::notes::{Note, NoteColor, NoteKind, NoteOrigin, StageState};
use std::collections::HashSet;

fn note(id: &str, extract: StageState) -> Note {
    Note {
        id: id.to_string(),
        created: String::new(),
        modified: String::new(),
        raw: String::new(),
        body: String::new(),
        extract_state: extract,
        origin: NoteOrigin::default(),
        kind: NoteKind::default(),
        color: NoteColor::Purple,
        archived: false,
    }
}

fn store(notes: Vec<Note>) -> NoteStore {
    NoteStore {
        notes,
        path: std::path::PathBuf::new(),
        dirty: false,
        machine: crate::notes::MachineStore::new(std::path::PathBuf::new()),
        doc: crate::notes::sync_doc::SyncHandle::default(),
        doc_dirty: false,
        load_error: None,
        unreadable_notes: Vec::new(),
    }
}

/// Marks a note archived (kept out of `note()`, which every other test uses as-is).
fn archived(mut n: Note) -> Note {
    n.archived = true;
    n
}

// ─── sweep_requests ────────────────────────────────────────────────────────

#[test]
fn every_swept_request_carries_the_swept_flag() {
    let notes = store(vec![
        note("a", StageState::Failed),
        note("b", StageState::Failed),
        note("c", StageState::Failed),
    ]);
    let reqs = sweep_requests(&notes, &HashSet::new());
    assert_eq!(reqs.len(), 3);
    assert!(reqs.iter().all(|r| r.swept), "a sweep that forgot the flag on even one request re-sweeps forever");
}

#[test]
fn a_mixed_backlog_sweeps_only_the_failed_notes() {
    let notes = store(vec![
        note("failed", StageState::Failed),
        note("healthy", StageState::Done),
        note("pending", StageState::Pending),
        note("skipped", StageState::Skipped),
    ]);
    let reqs = sweep_requests(&notes, &HashSet::new());
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].note_id, "failed");
}

#[test]
fn an_archived_notes_failed_pass_is_not_swept() {
    // Archiving means the user is done with the note. Without this filter its
    // `Failed` pass would be retried on every success, forever.
    let notes = store(vec![
        archived(note("gone", StageState::Failed)),
        note("active", StageState::Failed),
    ]);
    let reqs = sweep_requests(&notes, &HashSet::new());
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].note_id, "active");
}

#[test]
fn a_terminal_failure_is_not_swept() {
    // A failure that needs a config or server change (wrong model name,
    // broken preset) would fail identically on every sweep. Manual retry only.
    let notes = store(vec![note("a", StageState::Failed)]);
    let terminal: HashSet<String> = ["a".to_string()].into_iter().collect();
    assert!(sweep_requests(&notes, &terminal).is_empty());
}

// A pass switched off mid-flight must report `NotAttempted`, not `Errored`:
// nothing was learned about the server, and an error would suppress the sweep.

#[test]
fn abandoning_a_pass_reports_nothing_attempted_rather_than_a_failure() {
    let outcome = abandoned("extraction", "1a078edd");
    assert_eq!(outcome, RequestOutcome::NotAttempted);
    assert_ne!(
        outcome,
        RequestOutcome::Errored,
        "a pass the user switched off did not fail; calling it an error would \
         suppress the sweep for every other note in the same request"
    );
}

// ─── should_sweep ──────────────────────────────────────────────────────────

#[test]
fn a_swept_requests_completion_never_triggers_another_sweep() {
    // Without this guard a note that keeps failing would re-sweep the whole
    // backlog on every other note's success, forever.
    assert!(!should_sweep(true, true));
}

#[test]
fn an_ordinary_successful_request_does_trigger_a_sweep() {
    assert!(should_sweep(true, false));
}

#[test]
fn a_failed_request_never_triggers_a_sweep() {
    // A failure is no evidence the server is reachable.
    assert!(!should_sweep(false, false));
}

//! Pure tests for `pipeline.rs`. Split out under `#[path]` for the same
//! reason `task_store.rs` does: the parent file was already close to the
//! 500-line limit before this task's sweep logic landed.
//!
//! Everything here is a function of plain values (`NoteStore`, `StageState`,
//! `PipelineRequest`), with no coroutine, no signal and no server. That is
//! deliberate: there is no llama.cpp server reachable in CI or in this
//! sandbox, so the sweep's actual trigger (a real request completing inside
//! `use_pipeline`) is not something a test here can exercise. What is tested
//! is the two functions that trigger is built on: `sweep_requests`, which
//! decides what a sweep asks for, and `should_sweep`, which decides whether
//! one fires at all.

use super::*;
use crate::notes::{Note, NoteColor, NoteOrigin, StageState};
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

/// Marks a note archived, for the one test that needs it. A free function
/// rather than a `note()` parameter: every other test wants an active note,
/// and threading an unused `bool` through all of them would be noise.
fn archived(mut n: Note) -> Note {
    n.archived = true;
    n
}

#[test]
fn a_new_note_asks_for_a_pass() {
    let req = PipelineRequest::new("abc");
    assert_eq!(req.note_id, "abc");
    assert!(!req.swept, "a dictated note's own pass is never itself a sweep");
}

#[test]
fn requests_are_compared_by_note() {
    // The in-flight set keys on `note_id` alone, deliberately: a second
    // request for a note already being worked on is a duplicate, because
    // both would race writing the same suggestion rows.
    let a = PipelineRequest::new("n");
    let b = PipelineRequest { note_id: "n".into(), swept: true };
    assert_ne!(a, b);
    assert_eq!(a.note_id, b.note_id);
}

#[test]
fn retry_requests_are_never_swept() {
    let req = PipelineRequest::new("n");
    assert!(!req.swept, "the footer pressing retry is a person, not the sweep");
}

// ─── sweep_requests ────────────────────────────────────────────────────────

#[test]
fn nothing_failed_yields_nothing_to_sweep() {
    let notes = store(vec![
        note("a", StageState::Done),
        note("b", StageState::Skipped),
    ]);
    assert!(sweep_requests(&notes, &HashSet::new()).is_empty());
}

#[test]
fn a_failed_pass_is_swept() {
    let notes = store(vec![note("a", StageState::Failed)]);
    let reqs = sweep_requests(&notes, &HashSet::new());
    assert_eq!(reqs, vec![PipelineRequest { note_id: "a".into(), swept: true }]);
}

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
    // Archiving is the user saying they are done with the note. Without this
    // filter a `Failed` pass on an archived note would be retried forever,
    // once per success, for as long as the app runs.
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
    // A failure that needs a config or server change first (wrong model
    // name, broken preset) would fail identically on every sweep. The footer
    // still offers a manual retry; the sweep just leaves it alone.
    let notes = store(vec![note("a", StageState::Failed)]);
    let terminal: HashSet<String> = ["a".to_string()].into_iter().collect();
    assert!(sweep_requests(&notes, &terminal).is_empty());
}

// A pass abandoned mid-flight — the user switched it off while its request
// was in the air. What matters is that this reports `NotAttempted` and not
// `Errored`: nothing was learned about the server either way, and an
// `Errored` here would wrongly suppress the backlog sweep for a request that
// never actually failed.

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
    // The guard the brief calls out by name: without it, a note that keeps
    // failing would re-sweep the whole backlog on every other note's success,
    // forever.
    assert!(!should_sweep(true, true));
}

#[test]
fn an_ordinary_successful_request_does_trigger_a_sweep() {
    assert!(should_sweep(true, false));
}

#[test]
fn a_failed_request_never_triggers_a_sweep() {
    // Failure is not evidence the server is reachable. It is the opposite case
    // the brief's rationale rests on.
    assert!(!should_sweep(false, false));
}

#[test]
fn a_failed_swept_request_still_does_not_trigger_a_sweep() {
    assert!(!should_sweep(false, true));
}

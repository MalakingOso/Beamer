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
use crate::notes::{Note, NoteColor, NoteOrigin};

fn note(id: &str, clean: StageState, extract: StageState) -> Note {
    Note {
        id: id.to_string(),
        created: String::new(),
        modified: String::new(),
        raw: String::new(),
        body: String::new(),
        clean_state: clean,
        extract_state: extract,
        origin: NoteOrigin::default(),
        color: NoteColor::Purple,
        pos: None,
        size: None,
        attachments: Vec::new(),
        open: true,
        archived: false,
    }
}

fn store(notes: Vec<Note>) -> NoteStore {
    NoteStore { notes, path: std::path::PathBuf::new(), dirty: false }
}

#[test]
fn a_new_note_asks_for_both_stages() {
    let req = PipelineRequest::for_new_note("abc");
    assert_eq!(req.note_id, "abc");
    assert_eq!(
        req.stages,
        Stages::Both,
        "dictation is the one path where both passes run unasked"
    );
    assert!(!req.swept, "a dictated note's own pass is never itself a sweep");
}

#[test]
fn requests_are_compared_by_note_and_stages() {
    // The in-flight set keys on `note_id` alone, deliberately: a second
    // request for a note already being worked on is a duplicate whatever
    // stages it names, because both would race on the same CAS.
    let a = PipelineRequest::retry("n", Stages::Both);
    let b = PipelineRequest::retry("n", Stages::CleanOnly);
    assert_ne!(a, b);
    assert_eq!(a.note_id, b.note_id);
}

#[test]
fn retry_requests_are_never_swept() {
    let req = PipelineRequest::retry("n", Stages::Both);
    assert!(!req.swept, "the footer pressing retry is a person, not the sweep");
}

// ─── sweep_requests ────────────────────────────────────────────────────────

#[test]
fn nothing_failed_yields_nothing_to_sweep() {
    let notes = store(vec![
        note("a", StageState::Done, StageState::Done),
        note("b", StageState::Pending, StageState::Skipped),
    ]);
    assert!(sweep_requests(&notes).is_empty());
}

#[test]
fn a_failed_cleanup_alone_asks_for_clean_only() {
    let notes = store(vec![note("a", StageState::Failed, StageState::Done)]);
    let reqs = sweep_requests(&notes);
    assert_eq!(reqs, vec![PipelineRequest { note_id: "a".into(), stages: Stages::CleanOnly, swept: true }]);
}

#[test]
fn a_failed_extraction_alone_asks_for_extract_only() {
    let notes = store(vec![note("a", StageState::Done, StageState::Failed)]);
    let reqs = sweep_requests(&notes);
    assert_eq!(reqs, vec![PipelineRequest { note_id: "a".into(), stages: Stages::ExtractOnly, swept: true }]);
}

#[test]
fn both_stages_failed_asks_for_both() {
    let notes = store(vec![note("a", StageState::Failed, StageState::Failed)]);
    let reqs = sweep_requests(&notes);
    assert_eq!(reqs, vec![PipelineRequest { note_id: "a".into(), stages: Stages::Both, swept: true }]);
}

#[test]
fn every_swept_request_carries_the_swept_flag() {
    let notes = store(vec![
        note("a", StageState::Failed, StageState::Done),
        note("b", StageState::Done, StageState::Failed),
        note("c", StageState::Failed, StageState::Failed),
    ]);
    let reqs = sweep_requests(&notes);
    assert_eq!(reqs.len(), 3);
    assert!(reqs.iter().all(|r| r.swept), "a sweep that forgot the flag on even one request re-sweeps forever");
}

#[test]
fn a_mixed_backlog_asks_each_note_only_for_what_it_needs() {
    let notes = store(vec![
        note("clean-only", StageState::Failed, StageState::Done),
        note("healthy", StageState::Done, StageState::Done),
        note("extract-only", StageState::Skipped, StageState::Failed),
        note("both", StageState::Failed, StageState::Failed),
    ]);
    let reqs = sweep_requests(&notes);
    let by_id = |id: &str| reqs.iter().find(|r| r.note_id == id).map(|r| r.stages);
    assert_eq!(by_id("clean-only"), Some(Stages::CleanOnly));
    assert_eq!(by_id("healthy"), None);
    assert_eq!(by_id("extract-only"), Some(Stages::ExtractOnly));
    assert_eq!(by_id("both"), Some(Stages::Both));
    assert_eq!(reqs.len(), 3);
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

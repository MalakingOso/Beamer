//! Tests for [`super`].
//!
//! Split into their own file because `task_store.rs` reached 472 of the
//! project's 500-line limit with roughly 200 of that being tests, and the next
//! method could not be added without breaking the cap. Nothing here changed in
//! the move — this is the same suite, dedented one level.

use super::*;
use crate::notes::task::TaskKind;

/// A proposal with no date, which is what every pre-dating test assumed.
fn proposal(text: &str, evidence: &str, confidence: f32) -> Proposal {
    Proposal {
        text: text.into(),
        evidence: evidence.into(),
        confidence,
        ..Proposal::default()
    }
}

/// PID-scoped temp dir, distinct from the notes tests' dir so the two
/// cannot collide on a shared tag.
fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("beamer_tasks_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Constructed directly rather than via `load()`, which reads a fixed path
/// under the real config dir.
fn temp_store(tag: &str) -> TaskStore {
    let path = temp_dir().join(format!("{tag}.json"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("json.corrupt"));
    TaskStore { tasks: Vec::new(), path, ..TaskStore::detached() }
}

fn suggest(store: &mut TaskStore, note_id: &str, text: &str) -> String {
    let task = TaskStore::new_suggestion(note_id, proposal(text, text, 0.9), "test-machine");
    let id = task.id.clone();
    store.tasks.push(task);
    id
}

#[test]
fn dismissed_rows_are_retained_with_a_decided_timestamp() {
    let mut store = temp_store("dismiss");
    let id = suggest(&mut store, "note-1", "Call the vet");

    assert!(store.dismiss(&id), "a fresh suggestion must accept a decision");

    let task = store.tasks.iter().find(|t| t.id == id).expect(
        "dismissing must never delete — the row is the labelled negative the \
         eval corpus is built from",
    );
    assert_eq!(task.status, TaskStatus::Dismissed);
    assert!(
        task.decided.is_some(),
        "a decided row with no decision time cannot be used as a labelled example"
    );
    assert!(
        !store.is_dirty() && store.path.exists(),
        "a decision must reach disk immediately, not wait for a debounce tick \
         that a crash could cost us"
    );
}

#[test]
fn replace_suggestions_keeps_decided_rows() {
    let mut store = temp_store("replace");
    let decided = suggest(&mut store, "note-1", "Call the vet");
    let stale = suggest(&mut store, "note-1", "Buy milk maybe");
    let other = suggest(&mut store, "note-2", "Send the invoice");
    store.accept(&decided);

    let fresh = TaskStore::new_suggestion("note-1", proposal("Book a table", "book", 0.7), "test-machine");
    let fresh_id = fresh.id.clone();
    store.replace_suggestions("note-1", vec![fresh]);

    assert!(
        store.tasks.iter().any(|t| t.id == decided && t.status == TaskStatus::Accepted),
        "re-running extraction must not destroy a decision the user already made"
    );
    assert!(
        !store.tasks.iter().any(|t| t.id == stale),
        "an undecided proposal for this note is superseded, not duplicated"
    );
    assert!(
        store.tasks.iter().any(|t| t.id == other),
        "another note's chips must be untouched — clearing them would delete \
         corpus rows for a note nobody re-ran"
    );
    assert!(store.tasks.iter().any(|t| t.id == fresh_id));
    assert_eq!(store.suggested_for("note-1").len(), 1);
}

#[test]
fn a_corrupt_tasks_file_is_preserved_not_overwritten() {
    let path = temp_dir().join("corrupt.json");
    let backup = path.with_extension("json.corrupt");
    let _ = std::fs::remove_file(&backup);
    let garbage = "{ this is not json at all";
    std::fs::write(&path, garbage).unwrap();

    let store = TaskStore::load_from(path.clone());

    assert!(store.tasks.is_empty());
    assert!(!path.exists(), "the unreadable file is moved aside, not left to be overwritten");
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        garbage,
        "the original bytes must survive verbatim — accepts and dismissals are \
         not re-derivable, so a lost tasks.json is lost eval signal"
    );
}

#[test]
fn suggestions_made_in_the_same_millisecond_get_distinct_ids() {
    let a = TaskStore::new_suggestion("note-1", proposal("one", "one", 0.5), "test-machine");
    let b = TaskStore::new_suggestion("note-1", proposal("two", "two", 0.5), "test-machine");
    assert_ne!(
        a.id, b.id,
        "one extraction pass emits several tasks in a single instant; colliding \
         ids would make accept hit the wrong chip"
    );
}

#[test]
fn suggestion_ids_carry_the_machine_suffix() {
    let a = TaskStore::new_suggestion("note-1", proposal("one", "one", 0.5), "aaaa");
    let b = TaskStore::new_suggestion("note-1", proposal("one", "one", 0.5), "bbbb");
    assert!(
        a.id.ends_with("-aaaa") && b.id.ends_with("-bbbb"),
        "rows sync keyed by id, so two machines extracting in the same \
         millisecond must not mint the same id: {a:?} vs {b:?}",
        a = a.id,
        b = b.id
    );
}

#[test]
fn a_redundant_decision_neither_writes_nor_moves_the_timestamp() {
    let mut store = temp_store("redundant");
    let id = suggest(&mut store, "note-1", "Call the vet");
    store.accept(&id);
    let first = store.tasks[0].decided.clone();

    assert!(!store.dismiss(&id), "accept and dismiss are terminal");
    assert_eq!(store.tasks[0].status, TaskStatus::Accepted);
    assert_eq!(
        store.tasks[0].decided, first,
        "re-deciding must not rewrite the decision time the corpus depends on"
    );
}

#[test]
fn deciding_a_missing_id_does_not_flush_unrelated_changes() {
    let mut store = temp_store("missing");
    suggest(&mut store, "note-1", "Call the vet");
    store.dirty = true;

    assert!(!store.accept("no-such-task"));
    assert!(
        store.is_dirty() && !store.path.exists(),
        "a no-op decision must not trigger a write of whatever else happened to be pending"
    );
}

#[test]
fn accepted_excludes_suggested_and_dismissed_rows() {
    let mut store = temp_store("accepted");
    let yes = suggest(&mut store, "note-1", "Call the vet");
    let no = suggest(&mut store, "note-1", "Learn the piano");
    suggest(&mut store, "note-1", "Undecided");
    store.accept(&yes);
    store.dismiss(&no);

    let accepted: Vec<&str> = store.accepted().iter().map(|t| t.text.as_str()).collect();
    assert_eq!(
        accepted,
        vec!["Call the vet"],
        "only confirmed rows are tasks; nothing enters a task list unconfirmed"
    );
    assert_eq!(store.suggested_for("note-1").len(), 1);
}

#[test]
fn only_an_accepted_task_can_be_marked_done() {
    let mut store = temp_store("done");
    let pending = suggest(&mut store, "note-1", "Call the vet");
    store.flush_if_dirty();

    store.set_done(&pending, true);
    assert!(
        !store.tasks[0].done && !store.is_dirty(),
        "a suggestion is not a task yet, so it cannot be completed"
    );

    store.accept(&pending);
    store.set_done(&pending, true);
    assert!(store.tasks[0].done);
    assert!(store.is_dirty(), "a checkbox is debounced, not flushed inline");

    store.flush_if_dirty();
    store.set_done(&pending, true);
    assert!(!store.is_dirty(), "a redundant set_done must not schedule a write");
}

#[test]
fn tasks_round_trip_through_disk() {
    let mut store = temp_store("roundtrip");
    let id = suggest(&mut store, "note-7", "Call the vet");
    store.tasks[0].evidence = "yeah I need to call the vet about Biscuit".into();
    store.tasks[0].confidence = 0.82;
    store.dismiss(&id);

    let reloaded = TaskStore::load_from(store.path.clone());

    assert_eq!(reloaded.tasks.len(), 1);
    assert_eq!(reloaded.tasks[0].note_id, "note-7", "provenance must survive a restart");
    assert_eq!(reloaded.tasks[0].evidence, "yeah I need to call the vet about Biscuit");
    assert_eq!(reloaded.tasks[0].confidence, 0.82);
    assert_eq!(reloaded.tasks[0].status, TaskStatus::Dismissed);
    assert!(reloaded.tasks[0].decided.is_some());
    assert!(!store.path.with_extension("json.tmp").exists());
}

#[test]
fn confidence_is_stored_as_reported_not_clamped() {
    let out_of_range = TaskStore::new_suggestion("note-1", proposal("x", "x", 1.7), "test-machine");
    assert_eq!(
        out_of_range.confidence, 1.7,
        "an impossible confidence means the prompt or the parser misfired, and \
         quietly flattening it hides the one thing the corpus should show"
    );
}

#[test]
fn deleting_a_note_takes_its_rows_with_it() {
    // The one place a decided row may be removed. Everywhere else in this
    // store a decision is permanent corpus data.
    let mut store = temp_store("delete_for_note");
    let keep = suggest(&mut store, "note-2", "survives");
    let dismissed = suggest(&mut store, "note-1", "was dismissed");
    suggest(&mut store, "note-1", "still suggested");
    store.dismiss(&dismissed);

    assert_eq!(store.delete_for_note("note-1"), 2);

    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].id, keep);
    assert!(
        !store.is_dirty(),
        "the note's deletion is written at the same moment; leaving these out of \
         step across a crash would strand rows whose note is gone"
    );
    assert!(store.path.exists());
}

#[test]
fn deleting_a_note_with_no_rows_writes_nothing() {
    let mut store = temp_store("delete_for_note_empty");
    suggest(&mut store, "note-1", "unrelated");
    store.flush_if_dirty();

    assert_eq!(store.delete_for_note("note-2"), 0);
    assert!(!store.is_dirty());
}

#[test]
fn a_suggestion_carries_the_date_the_model_resolved() {
    let mut store = temp_store("dated");
    let row = TaskStore::new_suggestion(
        "note-1",
        Proposal {
            text: "Ring Sarah".into(),
            evidence: "ring Sarah before Friday".into(),
            confidence: 0.9,
            due: Some("2026-08-28".into()),
            due_all_day: true,
            due_phrase: Some("before Friday".into()),
            kind: TaskKind::Todo,
        },
        "test-machine",
    );
    store.tasks.push(row);

    let task = &store.tasks[0];
    assert_eq!(task.due.as_deref(), Some("2026-08-28"));
    assert!(task.due_all_day);
    assert_eq!(task.due_phrase.as_deref(), Some("before Friday"));
    assert_eq!(task.kind, TaskKind::Todo);
}

#[test]
fn setting_a_due_date_by_hand_keeps_the_phrase_the_model_saw() {
    let mut store = temp_store("set_due");
    let id = suggest(&mut store, "note-1", "sort the garage");
    store.tasks[0].due_phrase = Some("sometime next week".into());
    store.flush_if_dirty();

    assert!(store.set_due(&id, Some("2026-08-31".into()), true));

    let task = &store.tasks[0];
    assert_eq!(task.due.as_deref(), Some("2026-08-31"));
    assert!(task.due_all_day);
    assert_eq!(
        task.due_phrase.as_deref(),
        Some("sometime next week"),
        "the phrase records what the model saw; overwriting it erases the evidence \
         that it saw something it could not resolve"
    );
    assert!(!store.is_dirty(), "a date the user set is a decision and flushes inline");
}

#[test]
fn setting_the_same_due_date_twice_writes_nothing() {
    let mut store = temp_store("set_due_noop");
    let id = suggest(&mut store, "note-1", "x");
    store.set_due(&id, Some("2026-08-31".into()), true);
    store.flush_if_dirty();

    assert!(!store.set_due(&id, Some("2026-08-31".into()), true));
    assert!(!store.set_due("missing", Some("2026-08-31".into()), true));
    assert!(!store.is_dirty());
}

#[test]
fn a_task_written_before_dates_existed_loads_undated() {
    // Same free-migration mechanism as `Note`'s stage fields: no
    // `deny_unknown_fields`, every new field defaulted.
    let json = r#"{
        "id": "18f2-0001",
        "note_id": "note-1",
        "text": "Call the vet",
        "evidence": "I need to call the vet",
        "confidence": 0.93,
        "status": "accepted",
        "done": false,
        "created": "2026-08-01T09:15:00+01:00",
        "decided": "2026-08-01T09:16:00+01:00"
    }"#;
    let task: Task =
        serde_json::from_str(json).expect("an existing tasks.json must keep loading");

    assert_eq!(task.due, None);
    assert!(!task.due_all_day);
    assert_eq!(task.due_phrase, None);
    assert_eq!(task.kind, TaskKind::Todo, "an extracted commitment is a to-do");
}

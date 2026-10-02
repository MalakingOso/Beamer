use super::*;
use crate::notes::StageState;

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// Backdate a record the way a note made on another day would look.
fn made_on(store: &mut NoteStore, id: &str, when: &str) {
    store.notes.iter_mut().find(|n| n.id == id).unwrap().created = when.to_string();
}

#[test]
fn logging_trims_and_refuses_blank_text() {
    let mut store = NoteStore::default();
    assert!(store.log_accomplishment("   \n ").is_none());
    assert!(store.notes.is_empty());

    let id = store.log_accomplishment("  shipped the release  ").unwrap();
    assert_eq!(store.get(&id).unwrap().body, "shipped the release");
}

#[test]
fn an_accomplishment_is_not_a_note() {
    let mut store = NoteStore::default();
    let id = store.log_accomplishment("filed the taxes").unwrap();
    let note = store.get(&id).unwrap();
    assert_eq!(note.kind, NoteKind::Accomplishment);
    assert_eq!(note.extract_state, StageState::Skipped, "extraction must never pick it up");
    assert!(!store.is_open(&id), "it has no window");

    assert!(store.active().is_empty());
    assert!(store.archived().is_empty());
    assert!(store.search("taxes").is_empty());
    assert!(!store.any_active_open());
    store.set_all_open(true);
    assert!(!store.is_open(&id), "Show all must not open a window for it");
}

#[test]
fn notes_and_accomplishments_stay_apart() {
    let mut store = NoteStore::default();
    let note = store.create("buy milk".into(), NoteColor::Teal, NoteOrigin::Dictated);
    store.log_accomplishment("called the vet").unwrap();

    assert_eq!(store.active().len(), 1);
    assert_eq!(store.active()[0].id, note);
    let today = Local::now().date_naive();
    assert_eq!(store.accomplishments_on(today).len(), 1);
}

#[test]
fn a_day_lists_its_own_entries_in_the_order_they_happened() {
    let mut store = NoteStore::default();
    let a = store.log_accomplishment("first").unwrap();
    let b = store.log_accomplishment("second").unwrap();
    let c = store.log_accomplishment("from last week").unwrap();
    made_on(&mut store, &a, "2026-09-30T09:00:00+00:00");
    made_on(&mut store, &b, "2026-09-30T11:30:00+00:00");
    made_on(&mut store, &c, "2026-09-23T10:00:00+00:00");

    // Noon UTC is the same calendar day in any zone from UTC-11 to UTC+11.
    let rows: Vec<&str> = store
        .accomplishments_on(day(2026, 9, 30))
        .iter()
        .map(|n| n.body.as_str())
        .collect();
    assert_eq!(rows, ["first", "second"]);
    assert_eq!(store.accomplishment_days(), [day(2026, 9, 30), day(2026, 9, 23)]);
}

#[test]
fn editing_an_entry_does_not_move_it_to_another_day() {
    let mut store = NoteStore::default();
    let id = store.log_accomplishment("fixed the bug").unwrap();
    made_on(&mut store, &id, "2026-09-30T12:00:00+00:00");
    store.set_body(&id, "fixed the other bug".into());
    assert_eq!(store.accomplishments_on(day(2026, 9, 30)).len(), 1);
}

#[test]
fn a_row_with_an_unreadable_timestamp_is_in_no_day() {
    let mut store = NoteStore::default();
    let id = store.log_accomplishment("mystery").unwrap();
    made_on(&mut store, &id, "not a date");
    assert!(store.accomplishment_days().is_empty());
}

#[test]
fn deleting_removes_the_entry() {
    let mut store = NoteStore::default();
    let id = store.log_accomplishment("oops").unwrap();
    assert!(store.delete(&id));
    assert!(store.accomplishment_days().is_empty());
}

fn accepted_task(tasks: &mut TaskStore, text: &str) -> String {
    use crate::notes::task::Proposal;
    let proposal = Proposal { text: text.into(), ..Default::default() };
    let task = TaskStore::new_suggestion("note-1", proposal, "m");
    let id = task.id.clone();
    tasks.tasks.push(task);
    tasks.accept(&id);
    id
}

#[test]
fn a_ticked_task_joins_that_days_log_and_unticking_removes_it() {
    let mut notes = NoteStore::default();
    let mut tasks = TaskStore::default();
    let id = accepted_task(&mut tasks, "Call the vet");
    let today = Local::now().date_naive();

    assert!(log_for_day(&notes, &tasks, today).is_empty(), "not done yet");
    tasks.set_done(&id, true);
    notes.log_accomplishment("wrote the spec").unwrap();

    let log = log_for_day(&notes, &tasks, today);
    assert_eq!(log.len(), 2);
    assert_eq!(log.iter().filter(|e| e.is_task).count(), 1);
    assert_eq!(log_days(&notes, &tasks), [today]);

    tasks.set_done(&id, false);
    assert_eq!(log_for_day(&notes, &tasks, today).len(), 1);
}

#[test]
fn a_done_task_with_no_timestamp_has_no_day() {
    let mut tasks = TaskStore::default();
    let id = accepted_task(&mut tasks, "Old task");
    tasks.set_done(&id, true);
    tasks.tasks[0].completed = None; // ticked before `completed` existed
    assert!(tasks.completed_days().is_empty());
}

#[test]
fn a_task_is_dated_by_its_completion_not_its_creation() {
    let mut tasks = TaskStore::default();
    let id = accepted_task(&mut tasks, "Slow task");
    tasks.set_done(&id, true);
    tasks.tasks[0].created = "2026-01-01T00:00:00+00:00".into();
    tasks.tasks[0].completed = Some("2026-09-30T12:00:00+00:00".into());
    assert_eq!(tasks.completed_days(), [day(2026, 9, 30)]);
}

#[test]
fn the_summary_is_one_bullet_per_entry() {
    let mut store = NoteStore::default();
    let a = store.log_accomplishment("wrote the spec").unwrap();
    let b = store.log_accomplishment("reviewed\ntwo PRs").unwrap();
    made_on(&mut store, &a, "2026-09-30T09:00:00+00:00");
    made_on(&mut store, &b, "2026-09-30T10:00:00+00:00");
    let lines: Vec<&str> =
        store.accomplishments_on(day(2026, 9, 30)).iter().map(|n| n.body.as_str()).collect();
    assert_eq!(
        summary(day(2026, 9, 30), &lines),
        "Done on Wednesday, 30 September 2026\n- wrote the spec\n- reviewed two PRs"
    );
}

#[test]
fn a_record_written_before_kinds_existed_loads_as_a_note() {
    let json = r#"{"id":"1","created":"2026-01-01T00:00:00+00:00","modified":"2026-01-01T00:00:00+00:00",
        "raw":"hi","body":"hi","color":"purple","archived":false}"#;
    let note: Note = serde_json::from_str(json).unwrap();
    assert_eq!(note.kind, NoteKind::Note);
    assert!(note.is_note());
}

#[test]
fn kind_survives_the_document_and_plain_notes_add_no_key() {
    use crate::notes::doc_notes;
    use crate::notes::sync_doc::SyncDoc;
    use automerge::ReadDoc;

    let mut store = NoteStore::default();
    let note = store.create("buy milk".into(), NoteColor::Teal, NoteOrigin::Dictated);
    let done = store.log_accomplishment("shipped it").unwrap();

    let mut sync = SyncDoc::default();
    doc_notes::reconcile(&mut sync, &store.notes, &[]).unwrap();

    let hydrated = doc_notes::hydrate(&sync);
    let kind_of = |id: &str| hydrated.notes.iter().find(|n| n.id == id).unwrap().kind;
    assert_eq!(kind_of(&done), NoteKind::Accomplishment);
    assert_eq!(kind_of(&note), NoteKind::Note);

    // No `kind` key on a plain note: writing one would push an op per existing
    // note to every peer on upgrade.
    let root = sync.root_map_if_present("notes").unwrap();
    let (_, obj) = sync.doc().get(&root, note.as_str()).unwrap().unwrap();
    assert!(sync.doc().get(&obj, "kind").unwrap().is_none());
}

//! Tests for [`super`].
//!
//! Split into their own file when the page gained due chips, the
//! unresolved-phrase picker and calendar export: `tasks_page.rs` had about 124
//! lines of headroom under the project's 500-line limit and the additions did
//! not fit beside 180 lines of tests.

use super::*;

use crate::notes::task::{parse_due, TaskKind, TaskStatus};
use crate::notes::{NoteColor, NoteOrigin};

/// The day every date assertion resolves against. Fixed, so nothing here
/// depends on when the suite runs.
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, 23).unwrap()
}

fn task(note_id: &str, text: &str, created: &str, status: TaskStatus, done: bool) -> Task {
    Task {
        id: format!("{note_id}-{text}"),
        note_id: note_id.to_string(),
        text: text.to_string(),
        evidence: text.to_string(),
        confidence: 0.9,
        status,
        done,
        created: created.to_string(),
        decided: None,
        due: None,
        due_all_day: false,
        due_phrase: None,
        kind: TaskKind::Todo,
    }
}

fn texts(groups: &[(String, Vec<Task>)]) -> Vec<&str> {
    groups.iter().flat_map(|(_, rows)| rows).map(|t| t.text.as_str()).collect()
}

/// A `fn` rather than a closure: a closure here infers one lifetime for both
/// the borrow of the slice and the borrow inside it, and refuses to compile.
fn names<'a>(rows: &[&'a Task]) -> Vec<&'a str> {
    rows.iter().map(|t| t.text.as_str()).collect()
}

/// Which rows reach this page is `TaskStore::accepted`'s rule, pinned by
/// `accepted_excludes_suggested_and_dismissed_rows`. What is pinned here is
/// that grouping does not quietly re-sort what it was handed — the caller
/// owns newest-first order, and a sort re-added here would fight it.
#[test]
fn grouping_preserves_the_order_it_was_given() {
    let rows = vec![
        task("n1", "Newest", "2026-08-22T12:00:00+01:00", TaskStatus::Accepted, false),
        task("n1", "Oldest", "2026-08-22T09:00:00+01:00", TaskStatus::Accepted, false),
    ];
    assert_eq!(texts(&group_accepted(rows, today())), vec!["Newest", "Oldest"]);
}

#[test]
fn tasks_are_grouped_under_the_note_that_produced_them() {
    // Newest first, which is the order `TaskStore::accepted` hands over.
    let rows = vec![
        task("n2", "Send the invoice", "2026-08-22T11:00:00+01:00", TaskStatus::Accepted, false),
        task("n1", "Book a table", "2026-08-22T10:30:00+01:00", TaskStatus::Accepted, false),
        task("n1", "Call the vet", "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, false),
    ];
    let groups = group_accepted(rows, today());
    assert_eq!(groups.len(), 2, "one group per source note, not one row per task");
    assert_eq!(
        groups[0].0, "n2",
        "the note you last accepted from leads; grouping must not reshuffle to \
         note-creation order and bury the row just added"
    );
    assert_eq!(texts(&groups[1..]), vec!["Book a table", "Call the vet"]);
}

#[test]
fn done_tasks_sink_below_the_ones_still_outstanding() {
    let rows = vec![
        task("n1", "Done early", "2026-08-22T12:00:00+01:00", TaskStatus::Accepted, true),
        task("n1", "Still to do", "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, false),
    ];
    assert_eq!(
        texts(&group_accepted(rows, today())),
        vec!["Still to do", "Done early"],
        "a ticked row must stop competing for attention with work that is left"
    );
}

#[test]
fn a_ticked_task_stays_inside_its_own_note_group() {
    let rows = vec![
        task("n2", "Open", "2026-08-22T11:00:00+01:00", TaskStatus::Accepted, false),
        task("n1", "Done", "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, true),
    ];
    let groups = group_accepted(rows, today());
    assert_eq!(groups.len(), 2);
    assert_eq!(
        texts(&groups[1..]),
        vec!["Done"],
        "a note's tasks always move together — a group is either in the main list \
         or in the page-level Completed section, never split across the two, \
         because the group is the provenance this page exists to show"
    );
}

#[test]
fn a_group_is_finished_only_when_every_row_in_it_is_done() {
    let done = |text: &str, done| task("n1", text, "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, done);
    assert!(group_finished(&[done("A", true), done("B", true)]));
    assert!(
        !group_finished(&[done("A", true), done("B", false)]),
        "one row left to do keeps the whole note in the main list"
    );
}

#[test]
fn an_empty_group_is_not_finished() {
    // `all()` is true on an empty slice, so the guard in `group_finished` is
    // the only thing between a note with no tasks at all and a promotion into
    // the Completed section, where it would read as work that was done.
    assert!(!group_finished(&[]));
}

#[test]
fn a_half_done_note_stays_put_while_a_finished_one_is_promoted() {
    // The behaviour actually asked for, and the one a later refactor is most
    // likely to break: promotion is decided per group, so a note with anything
    // outstanding keeps its place even when another note is entirely ticked.
    let mut half = dated("Half done", Some("2026-08-24"), true, false);
    half.note_id = "n2".into();
    let mut half_ticked = dated("Half ticked", Some("2026-08-01"), true, true);
    half_ticked.note_id = "n2".into();

    let groups = group_accepted(
        vec![half, half_ticked, dated("All done", Some("2026-08-01"), true, true)],
        today(),
    );

    let (active, finished): (Vec<_>, Vec<_>) =
        groups.iter().partition(|(_, rows)| !group_finished(rows));

    assert_eq!(active.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["n2"]);
    assert_eq!(finished.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["n1"]);
}

#[test]
fn un_ticking_the_last_done_task_returns_a_group_to_the_main_list() {
    let mut rows = vec![
        dated("Done", Some("2026-08-01"), true, true),
        dated("Also done", Some("2026-08-02"), true, true),
    ];
    assert!(group_finished(&rows), "both ticked, so the note has graduated");

    rows[1].done = false;
    assert!(
        !group_finished(&rows),
        "un-ticking a row must bring the whole group back — the Completed section \
         is derived from the rows, never a flag set at promotion time"
    );
}

#[test]
fn the_completed_disclosure_splits_a_group_without_losing_a_row() {
    let rows = group_accepted(
        vec![
            dated("Done late", Some("2026-08-01"), true, true),
            dated("Due tomorrow", Some("2026-08-24"), true, false),
            dated("Done early", Some("2026-08-02"), true, true),
            dated("Undated", None, false, false),
        ],
        today(),
    )
    .remove(0)
    .1;

    let (outstanding, done) = split_done(&rows);

    assert_eq!(
        names(&outstanding),
        vec!["Due tomorrow", "Undated"],
        "the visible half must keep the overdue-first order the group was sorted into"
    );
    assert_eq!(names(&done), vec!["Done late", "Done early"]);
    assert_eq!(
        outstanding.len() + done.len(),
        rows.len(),
        "a task that lands in neither half is a commitment silently dropped from the page"
    );
}

#[test]
fn a_group_with_nothing_left_to_do_is_promoted_whole() {
    // This test used to say the group keeps an inner disclosure, on the
    // grounds that without one a fully-ticked note is a bare heading with no
    // way to reach its rows. That is now the *intended* rendering: the group
    // leaves the main list entirely and its rows show directly under the
    // page-level Completed section, so an inner caret would be a control with
    // nothing left to hide.
    //
    // What still has to hold is that the rows survive the move. `split_done`
    // puts every one of them in the done half, which is the half the promoted
    // rendering draws — so a group cannot lose a row by being promoted.
    let rows = vec![dated("Done", Some("2026-08-01"), true, true)];
    let (outstanding, done) = split_done(&rows);
    assert!(outstanding.is_empty());
    assert_eq!(done.len(), 1, "the only way back to these rows is the disclosure");
    assert!(group_finished(&rows), "and nothing outstanding is what triggers the move");
}

#[test]
fn a_task_whose_note_is_gone_still_gets_a_heading() {
    let h = heading_for(None);
    assert!(!h.openable, "there is no window to open for a note that no longer exists");
    assert_eq!(h.badge, Some("deleted"));
    assert_eq!(
        h.stripe, "slate",
        "an unattributed group must not borrow another note's colour as an index"
    );
}

#[test]
fn an_archived_notes_heading_resolves_but_does_not_offer_a_click() {
    let mut store = NoteStore::default();
    let id = store.create("call the vet".into(), NoteColor::Teal, NoteOrigin::Dictated);
    store.archive(&id);
    let h = heading_for(store.get(&id));
    assert_eq!(h.text, "call the vet", "an archived note is still the task's provenance");
    assert_eq!(h.stripe, "teal");
    assert!(
        !h.openable,
        "the reconciler only opens a note that is open and not archived, so a click \
         here would look live and silently do nothing"
    );
}

#[test]
fn a_long_note_heading_is_trimmed_and_marked() {
    let h = heading_preview(&"word ".repeat(50));
    assert!(h.ends_with('\u{2026}'), "a trimmed heading must say so: {h}");
    assert!(h.chars().count() <= HEADING_CHARS + 1);
}

#[test]
fn a_heading_collapses_the_whitespace_a_transcript_carries() {
    assert_eq!(heading_preview("call   the\n\nvet  "), "call the vet");
}

#[test]
fn an_empty_note_is_named_rather_than_left_blank() {
    assert_eq!(
        heading_preview("   \n "),
        "Untitled note",
        "a blank heading would leave its tasks looking unattributed"
    );
}

fn dated(text: &str, due: Option<&str>, all_day: bool, done: bool) -> Task {
    let mut t = task("n1", text, "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, done);
    t.due = due.map(str::to_string);
    t.due_all_day = all_day;
    t
}

#[test]
fn a_near_date_reads_as_a_word_and_a_far_one_as_a_date() {
    // "In 9 days" is not something anyone can plan against, and "23 Aug" is
    // not what the user said about tomorrow. The switch is the point.
    let day = |s: &str| parse_due(s, true).unwrap();
    assert_eq!(due_label(day("2026-08-23"), today()), "Today");
    assert_eq!(due_label(day("2026-08-24"), today()), "Tomorrow");
    assert_eq!(due_label(day("2026-08-22"), today()), "Yesterday");
    assert_eq!(due_label(day("2026-08-27"), today()), "Thursday");
    assert_eq!(due_label(day("2026-09-15"), today()), "15 Sep");
    assert_eq!(
        due_label(day("2027-01-04"), today()),
        "4 Jan 2027",
        "a date in another year must say which"
    );
}

#[test]
fn a_timed_due_shows_its_time() {
    let at = parse_due("2026-08-25T09:00:00", false).unwrap();
    assert_eq!(due_label(at, today()), "Tuesday 09:00");
}

#[test]
fn overdue_work_leads_and_the_most_overdue_leads_it() {
    let rows = vec![
        dated("Undated", None, false, false),
        dated("Next week", Some("2026-08-30"), true, false),
        dated("Late", Some("2026-08-20"), true, false),
        dated("Very late", Some("2026-08-10"), true, false),
        dated("Ticked", Some("2026-08-01"), true, true),
        dated("Tomorrow", Some("2026-08-24"), true, false),
    ];
    assert_eq!(
        texts(&group_accepted(rows, today())),
        vec!["Very late", "Late", "Tomorrow", "Next week", "Undated", "Ticked"],
        "a list that knows what is due tomorrow and shows it fourth has to be read in full"
    );
}

#[test]
fn a_ticked_task_never_counts_as_overdue() {
    let done = dated("Done late", Some("2026-01-01"), true, true);
    assert!(
        !done.is_overdue(today()),
        "reddening finished work would train the user to ignore the colour"
    );
    assert!(dated("Still late", Some("2026-01-01"), true, false).is_overdue(today()));
    assert!(!dated("Undated", None, false, false).is_overdue(today()));
}

#[test]
fn a_task_due_today_is_not_overdue() {
    // Days, not instants: something due at 09:00 today is not late at 17:00 in
    // the sense the word is being used here.
    assert!(!dated("Today", Some("2026-08-23T09:00:00"), false, false).is_overdue(today()));
}

#[test]
fn the_date_picker_understands_both_shapes_and_an_empty_field() {
    assert_eq!(picked_due("2026-08-31"), (Some("2026-08-31".to_string()), true));
    assert_eq!(
        picked_due("2026-08-31T09:30"),
        (Some("2026-08-31T09:30".to_string()), false)
    );
    assert_eq!(
        picked_due("  "),
        (None, false),
        "clearing the field must clear the date, not store an empty string"
    );
}

#[test]
fn grouping_still_keeps_a_dated_row_under_its_own_note() {
    // The provenance the page exists for outranks the date: sorting is within
    // a group, never across groups.
    let mut other = dated("Other note, due sooner", Some("2026-08-24"), true, false);
    other.note_id = "n2".into();
    let rows = vec![dated("This note, due later", Some("2026-09-30"), true, false), other];

    let groups = group_accepted(rows, today());
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].0, "n1", "the caller's newest-first order still owns the groups");
}

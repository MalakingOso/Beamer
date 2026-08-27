//! The Tasks page — every accepted task, grouped by the note it came from.
//!
//! **Dismissed rows are deliberately absent, and are not missing.** A dismissed
//! suggestion stays in `tasks.json` forever as a labelled negative: it is the
//! record that the model proposed something and a human said no, which is the
//! scarce half of the eval corpus that will later measure extraction precision.
//! It is not a task, so it is not on the Tasks page. Nothing here — and nothing
//! that "cleans up" this page later — may delete one. Undecided suggestions are
//! absent for the other half of the same rule: nothing enters a task list
//! unconfirmed, so a proposal lives on its note's chips until it is decided.
//!
//! Grouping by note rather than by day is the provenance payoff. A task read
//! alone is a bare imperative; read under the note that produced it, a wrong
//! extraction is obvious, and the heading is a click back to the note itself.
//!
//! Store and registry arrive as **props**, for the reason `notes_page.rs`
//! documents: nothing in `src/` calls `provide_context`, so `use_context` here
//! would panic rather than resolve.

use std::collections::HashSet;

use dioxus::prelude::*;

use chrono::{Datelike, NaiveDate};

use crate::notes::task::{Due, Task};
use crate::notes::task_store::TaskStore;
use crate::notes::{Note, NoteStore};
use crate::ui::sticky_windows::StickyRegistry;

mod group;
mod rows;
use group::TaskGroup;

#[derive(Props, Clone, PartialEq)]
pub struct TasksPageProps {
    pub notes: Signal<NoteStore>,
    pub tasks: Signal<TaskStore>,
    pub registry: StickyRegistry,
}

/// How much of a note's body a group heading shows.
///
/// Much shorter than the notes board's 180: this is a one-line label above a
/// list, not the card that has to stand in for the note itself.
const HEADING_CHARS: usize = 60;

fn heading_preview(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return "Untitled note".to_string();
    }
    if flat.chars().count() <= HEADING_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(HEADING_CHARS).collect();
    format!("{}\u{2026}", cut.trim_end())
}

/// How one group identifies the note that produced it.
#[derive(Debug, Clone, PartialEq)]
struct Heading {
    text: String,
    /// Suffix for `note-stripe-{}`.
    stripe: &'static str,
    /// Whether clicking the heading opens the note's window.
    openable: bool,
    /// Muted word explaining why a heading is not openable.
    badge: Option<&'static str>,
}

/// Three cases, not two.
///
/// `NoteStore::get` searches archived notes as well, so `None` means the note
/// is genuinely gone — hand-edited out of `notes.json`, or lost to the
/// quarantine path. Tasks outlive their notes on purpose, so the rows are still
/// listed: dropping a commitment the user accepted because its provenance
/// vanished would be a silent data loss, and an obviously-unattributed group is
/// the honest rendering.
///
/// An archived note resolves fine but is **not** openable: `reopen_note` sets
/// `open = true`, while the reconciler only opens a note that is
/// `open && !archived`, so the click would look live and do nothing. Say so
/// with a badge instead of offering a gesture that no-ops.
fn heading_for(note: Option<&Note>) -> Heading {
    match note {
        None => Heading {
            text: "Note unavailable".to_string(),
            stripe: "slate",
            openable: false,
            badge: Some("deleted"),
        },
        Some(n) if n.archived => Heading {
            text: heading_preview(&n.body),
            stripe: n.color.css_class(),
            openable: false,
            badge: Some("archived"),
        },
        Some(n) => Heading {
            text: heading_preview(&n.body),
            stripe: n.color.css_class(),
            openable: true,
            badge: None,
        },
    }
}

/// How a due date reads on a chip.
///
/// Relative wording for the near days, because "tomorrow" is what the user
/// said and what they will look for. Beyond that an absolute date, because
/// "in 9 days" is not something anyone can plan against.
pub fn due_label(due: Due, today: NaiveDate) -> String {
    let day = due.date();
    let time = match due {
        Due::At(dt) => format!(" {}", dt.format("%H:%M")),
        Due::AllDay(_) => String::new(),
    };
    let days = (day - today).num_days();
    let when = match days {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        // Inside the coming week a weekday name is the most useful form, and
        // it is unambiguous because it can only mean the next one.
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == today.year() => day.format("%-d %b").to_string(),
        _ => day.format("%-d %b %Y").to_string(),
    };
    format!("{when}{time}")
}

/// What an `<input type="date">` / `datetime-local"` value means for the store.
///
/// The picker submits `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM`; an empty value means
/// the user cleared it, which must clear the date rather than store `""`.
pub fn picked_due(value: &str) -> (Option<String>, bool) {
    let value = value.trim();
    if value.is_empty() {
        return (None, false);
    }
    let all_day = !value.contains('T');
    (Some(value.to_string()), all_day)
}

/// Order rows within a note: overdue first, then by date, then undated, then
/// done — and stable inside each band, so the caller's newest-first survives.
///
/// Sorting by date rather than only by `done` is the point of dating tasks at
/// all: a list that knows what is due tomorrow and shows it fourth is a list
/// that has to be read in full anyway.
fn due_rank(task: &Task, today: NaiveDate) -> (u8, i64) {
    if task.done {
        return (3, 0);
    }
    match task.due_parsed() {
        Some(due) => {
            let days = (due.date() - today).num_days();
            // Overdue leads, and the *most* overdue leads it.
            (if days < 0 { 0 } else { 1 }, days)
        }
        // Undated work is not urgent by default and is not buried either.
        None => (2, 0),
    }
}

/// Group already-accepted, already-sorted rows by their note.
///
/// The filter and the newest-first sort belong to `TaskStore::accepted`, not
/// here. This used to repeat both, with a comment saying it matched the store —
/// two implementations of one rule, and a comment is not a mechanism to keep
/// them in step. `rows` arrives filtered; grouping is all that is left, and it
/// is the part worth testing.
///
/// Done tasks sink to the bottom of their group, which is what lets the render
/// split them off behind a disclosure *inside* that group. A group whose tasks
/// are **all** done graduates to the page-level Completed section instead — but
/// as a whole group, never row by row, because the group is the note and
/// splitting a note's tasks across two places would cost the provenance this
/// page exists for. See `group_finished`. The sort is stable, so ticking a box
/// moves one row and leaves everything else where the eye last saw it.
fn group_accepted(rows: Vec<Task>, today: NaiveDate) -> Vec<(String, Vec<Task>)> {
    let mut groups: Vec<(String, Vec<Task>)> = Vec::new();
    for task in rows {
        match groups.iter_mut().find(|(id, _)| *id == task.note_id) {
            Some((_, bucket)) => bucket.push(task),
            None => groups.push((task.note_id.clone(), vec![task])),
        }
    }
    for (_, bucket) in groups.iter_mut() {
        bucket.sort_by_key(|t| due_rank(t, today));
    }
    groups
}

/// Split one group's rows into the work that is left and the work that is done.
///
/// A function rather than a `partition` inlined into the render, so the rule is
/// testable: the split is the only thing standing between a ticked task and
/// disappearing from the page, and "it looked right when I clicked it" is not a
/// check that survives the next edit.
///
/// `partition` is stable, so both halves keep the order `group_accepted` gave
/// them — the outstanding half stays overdue-first, and the completed half
/// stays in the order the rows were accepted.
fn split_done(rows: &[Task]) -> (Vec<&Task>, Vec<&Task>) {
    rows.iter().partition(|t| !t.done)
}

/// Whether a note has nothing left outstanding, and so belongs in the
/// page-level Completed section rather than the main list.
///
/// The emptiness guard is not defensive noise: `all()` on an empty slice is
/// `true`, so without it a group with no rows would report itself finished and
/// promote a note that has no tasks at all. `group_accepted` cannot currently
/// produce one — every group is created by pushing a task into it — but the
/// rule should not depend on that staying true.
fn group_finished(rows: &[Task]) -> bool {
    !rows.is_empty() && rows.iter().all(|t| t.done)
}

#[component]
pub fn TasksPage(props: TasksPageProps) -> Element {
    let notes = props.notes;
    let tasks = props.tasks;
    let registry = props.registry;

    // Cloned into owned rows rather than held as borrows: the controls below
    // capture their ids in click handlers, which cannot outlive a `read()`
    // guard on the store. Same reasoning as `notes_page`.
    // Read inside the memo, not beside it. A `use_memo` closure is created once
    // and never recreated, so a date captured out here would freeze at first
    // render — a page left open across midnight would sort against yesterday
    // while the chips below labelled against today.
    let groups = use_memo(move || {
        let today = chrono::Local::now().date_naive();
        let accepted: Vec<Task> =
            tasks.read().accepted().into_iter().cloned().collect();
        let store = notes.read();
        group_accepted(accepted, today)
            .into_iter()
            .map(|(id, rows)| {
                let heading = heading_for(store.get(&id));
                (id, heading, rows)
            })
            .collect::<Vec<_>>()
    });

    // The render body's own read, for the chips. Same day as the memo's in
    // every case that matters; both are re-evaluated when the store changes.
    let today = chrono::Local::now().date_naive();

    // Which groups have their completed section open, keyed by note id.
    //
    // One page-level set rather than a signal per group: a hook cannot be
    // created inside the `for` below without breaking Dioxus' call order, and
    // that failure compiles and then misbehaves rather than erroring.
    //
    // Keyed by note id rather than by position because accepting a task
    // reshuffles the groups into newest-first order — an index would leave the
    // open section attached to whichever note slid into that slot.
    //
    // Deliberately not persisted. Finished work collapses again on every visit,
    // which is the point of moving it out of the way at all.
    // Written by `TaskGroup`, never here — the page owns the set so the hook
    // is created once, outside the loop, but only a group's own caret mutates
    // it. Hence no `mut` binding.
    let expanded = use_signal(HashSet::<String>::new);

    // Whether the page-level Completed section is open.
    //
    // Separate from `expanded` above, and deliberately not merged into it: that
    // set answers "is this note's inner caret open", keyed by note id, and there
    // is no note id for the page-level section. Not persisted, same as
    // `expanded`.
    let mut show_finished = use_signal(|| false);

    let outstanding =
        groups.read().iter().flat_map(|(_, _, rows)| rows).filter(|t| !t.done).count();
    let is_empty = groups.read().is_empty();

    // Owned tuples, taken by cloning the memo rather than borrowing it: the
    // props below and the click handlers inside them outlive any `read()`
    // guard, so partitioning the guard's contents would not compile.
    let (active, finished): (Vec<_>, Vec<_>) =
        groups().into_iter().partition(|(_, _, rows)| !group_finished(rows));
    let finished_count = finished.len();

    rsx! {
        div { class: "content",
            div { class: "notes-header",
                h1 { class: "notes-title", "Tasks" }
                {
                    let label = match outstanding {
                        0 if is_empty => "No tasks".to_string(),
                        0 => "All done".to_string(),
                        1 => "1 to do".to_string(),
                        n => format!("{n} to do"),
                    };
                    rsx! { span { class: "notes-count", "{label}" } }
                }
            }

            p { class: "notes-description",
                "Suggestions you accepted, kept under the note they came from. Click a heading to open that note."
            }

            if is_empty {
                div { class: "empty-state",
                    span { class: "empty-state-text", "Nothing accepted yet" }
                    span { class: "empty-state-hint",
                        // Not an error and not a setup step. Most notes contain
                        // no commitment at all, and extraction proposing
                        // nothing is the correct outcome far more often than
                        // not — the wording must not suggest something failed.
                        "Most notes hold no tasks. When one does, accept the suggestion on the note and it lands here."
                    }
                }
            } else {
                for (note_id, heading, rows) in active.iter() {
                    TaskGroup {
                        key: "{note_id}",
                        note_id: note_id.clone(),
                        heading: heading.clone(),
                        rows: rows.clone(),
                        tasks,
                        notes,
                        registry,
                        today,
                        finished: false,
                        expanded,
                    }
                }

                if finished_count > 0 {
                    // Rendered as a sibling of the groups rather than a box
                    // around them, so `.content`'s own gap spaces the revealed
                    // groups exactly as it spaces the main list.
                    button {
                        class: "tasks-completed-toggle",
                        title: if show_finished() { "Hide finished notes" } else { "Show finished notes" },
                        onclick: move |_| {
                            let open = show_finished();
                            show_finished.set(!open);
                        },
                        span { class: "task-done-chevron",
                            if show_finished() { "\u{25BE}" } else { "\u{25B8}" }
                        }
                        if finished_count == 1 {
                            "Completed \u{00b7} 1 note"
                        } else {
                            "Completed \u{00b7} {finished_count} notes"
                        }
                    }
                    if show_finished() {
                        for (note_id, heading, rows) in finished.iter() {
                            TaskGroup {
                                key: "{note_id}",
                                note_id: note_id.clone(),
                                heading: heading.clone(),
                                rows: rows.clone(),
                                tasks,
                                notes,
                                registry,
                                today,
                                finished: true,
                                expanded,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tasks_page/tests.rs"]
mod tests;

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

use dioxus::prelude::*;

use chrono::{Datelike, NaiveDate};

use crate::notes::ics;
use crate::notes::task::{Due, Task};
use crate::notes::task_store::TaskStore;
use crate::notes::{Note, NoteStore};
use crate::ui::icons::IconCheck;
use crate::ui::sticky_windows::{self, StickyRegistry};

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
/// Done tasks sink to the bottom of their group rather than to a separate
/// section: the group is the note, and splitting a note's tasks across two
/// places would cost the provenance this page exists for. The sort is stable,
/// so ticking a box moves one row down and leaves everything else where the
/// eye last saw it.
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

/// The dated half of a task row: a chip, or the phrase the model could not
/// resolve beside a picker.
///
/// Three states, and the middle one is the whole reason `due_phrase` exists:
///
/// | State | Shown |
/// |---|---|
/// | Resolved | A date chip, red when overdue, and **Add to calendar** |
/// | Unresolved phrase | The words the model saw, and a date input |
/// | No timing mentioned | Nothing |
///
/// Nothing here reaches a calendar on its own. Export is a click, per task,
/// which follows from the standing decision that tasks are suggestions and
/// nothing is ever added unconfirmed.
#[derive(Props, Clone, PartialEq)]
struct DueRowProps {
    task: Task,
    tasks: Signal<TaskStore>,
    today: NaiveDate,
}

#[component]
fn DueRow(props: DueRowProps) -> Element {
    let DueRowProps { task, mut tasks, today } = props;
    let id = task.id.clone();

    if let Some(due) = task.due_parsed() {
        let overdue = task.is_overdue(today);
        let export = task.clone();
        return rsx! {
            div { class: "task-due-row",
                span {
                    class: if overdue { "task-due overdue" } else { "task-due" },
                    title: "{task.due.clone().unwrap_or_default()}",
                    "{due_label(due, today)}"
                }
                button {
                    class: "task-due-btn",
                    title: "Write an .ics and hand it to your calendar",
                    onclick: move |_| match ics::write_temp(&export) {
                        // One-way: the calendar imports a copy. Beamer cannot
                        // edit or remove it afterwards — see `notes::ics`.
                        Ok(path) => crate::ui::open_external(&path.to_string_lossy()),
                        Err(e) => tracing::warn!("Could not write a calendar file: {}", e),
                    },
                    "Add to calendar"
                }
            }
        };
    }

    let Some(phrase) = task.due_phrase.clone() else {
        return rsx! {};
    };

    rsx! {
        div { class: "task-due-row",
            // What the model saw and could not turn into a date. Shown rather
            // than swallowed: it is the difference between a picker you know
            // what to fill in and a blank field you have to re-read the note for.
            span { class: "task-due unresolved", title: "Beamer would not guess a date for this",
                "\u{201c}{phrase}\u{201d}"
            }
            input {
                class: "task-due-input",
                r#type: "date",
                title: "Set a date yourself",
                onchange: move |e: Event<FormData>| {
                    let (due, all_day) = picked_due(&e.value());
                    tasks.write().set_due(&id, due, all_day);
                },
            }
        }
    }
}

#[component]
pub fn TasksPage(props: TasksPageProps) -> Element {
    let notes = props.notes;
    let mut tasks = props.tasks;
    let registry = props.registry;

    // Cloned into owned rows rather than held as borrows: the controls below
    // capture their ids in click handlers, which cannot outlive a `read()`
    // guard on the store. Same reasoning as `notes_page`.
    // Read once per render rather than per row: every chip and every sort
    // compares against the same day, and a boundary crossed mid-render would
    // show two rows disagreeing about what "today" is.
    let today = chrono::Local::now().date_naive();

    let groups = use_memo(move || {
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

    let outstanding =
        groups.read().iter().flat_map(|(_, _, rows)| rows).filter(|t| !t.done).count();
    let is_empty = groups.read().is_empty();

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
                for (note_id, heading, rows) in groups.read().iter() {
                    div { key: "{note_id}", class: "task-group",
                        div {
                            class: if heading.openable { "task-group-header openable" } else { "task-group-header" },
                            title: if heading.openable { "Open this note" } else { "" },
                            onclick: {
                                let id = note_id.clone();
                                let openable = heading.openable;
                                move |_| {
                                    if openable {
                                        sticky_windows::reopen_note(registry, notes, &id);
                                    }
                                }
                            },
                            div { class: "note-stripe note-stripe-{heading.stripe}" }
                            span { class: "task-group-title", "{heading.text}" }
                            if let Some(badge) = heading.badge {
                                span { class: "task-group-badge", "{badge}" }
                            }
                        }
                        for task in rows.iter() {
                            {
                                let id = task.id.clone();
                                let done = task.done;
                                rsx! {
                                    div {
                                        key: "{id}",
                                        class: if done { "task-row done" } else { "task-row" },
                                        button {
                                            class: if done { "task-check checked" } else { "task-check" },
                                            title: if done { "Mark as not done" } else { "Mark as done" },
                                            onclick: {
                                                let id = id.clone();
                                                move |_| { tasks.write().set_done(&id, !done); }
                                            },
                                            if done {
                                                IconCheck { size: 12 }
                                            }
                                        }
                                        div { class: "task-row-main",
                                            div { class: "task-text", "{task.text}" }
                                            DueRow { task: task.clone(), tasks, today }
                                            // Full span on hover: it is one
                                            // ellipsized line, and being able to
                                            // check it is the whole point.
                                            div {
                                                class: "task-evidence",
                                                title: "{task.evidence}",
                                                "\u{201c}{task.evidence}\u{201d}"
                                            }
                                        }
                                    }
                                }
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

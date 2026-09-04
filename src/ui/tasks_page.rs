//! The Tasks page — accepted tasks, grouped by the note they came from (a
//! heading click opens that note). Dismissed rows are absent on purpose: they
//! stay in `tasks.json` as labelled negatives for the eval corpus and must
//! never be deleted. Undecided proposals live on their note's chips.
//!
//! Store and registry arrive as props (`notes_page.rs` explains why).

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

/// How much of a note's body a one-line group heading shows.
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

/// Three cases: `None` means genuinely gone (`get` covers archived notes), so
/// orphaned rows still list under a "deleted" badge rather than vanishing.
/// Archived notes resolve but are not openable — the reconciler only opens
/// `open && !archived`, so a click would no-op; badge instead.
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

/// How a due date reads on a chip: relative for near days, absolute beyond.
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
        // Weekday names within the coming week can only mean the next one.
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == today.year() => day.format("%-d %b").to_string(),
        _ => day.format("%-d %b %Y").to_string(),
    };
    format!("{when}{time}")
}

/// Parse a date/datetime-local picker value. Empty means the user cleared it,
/// which must clear the date rather than store `""`.
pub fn picked_due(value: &str) -> (Option<String>, bool) {
    let value = value.trim();
    if value.is_empty() {
        return (None, false);
    }
    let all_day = !value.contains('T');
    (Some(value.to_string()), all_day)
}

/// Row order within a note: overdue first, then by date, undated, done.
/// Stable inside each band, so the caller's newest-first survives.
fn due_rank(task: &Task, today: NaiveDate) -> (u8, i64) {
    if task.done {
        return (3, 0);
    }
    match task.due_parsed() {
        Some(due) => {
            let days = (due.date() - today).num_days();
            (if days < 0 { 0 } else { 1 }, days)
        }
        None => (2, 0),
    }
}

/// Group already-accepted, already-sorted rows by note (`TaskStore::accepted`
/// owns filtering and order; this only groups). Done rows sink within their
/// group behind a disclosure; an all-done group graduates whole to the
/// page-level Completed section, never row by row. Stable sort, so ticking a
/// box moves one row and leaves the rest in place.
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

/// Split one group's rows into outstanding and done. Stable, so both halves
/// keep the order `group_accepted` gave them.
fn split_done(rows: &[Task]) -> (Vec<&Task>, Vec<&Task>) {
    rows.iter().partition(|t| !t.done)
}

/// Whether a note has nothing outstanding and belongs in Completed. The
/// emptiness guard matters: `all()` on an empty slice is `true`.
fn group_finished(rows: &[Task]) -> bool {
    !rows.is_empty() && rows.iter().all(|t| t.done)
}

#[component]
pub fn TasksPage(props: TasksPageProps) -> Element {
    let notes = props.notes;
    let tasks = props.tasks;
    let registry = props.registry;

    // Owned rows: handlers cannot outlive a `read()` guard. Date read inside
    // the memo, not beside it — a captured date would freeze at first render.
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

    // Same day as the memo's in every case that matters.
    let today = chrono::Local::now().date_naive();

    // Open disclosures, keyed by note id (not position: accepting a task
    // reshuffles groups). One page-level set because hooks can't be created
    // inside the `for` below. Written by `TaskGroup`, never here. Not persisted.
    let expanded = use_signal(HashSet::<String>::new);

    // Page-level Completed disclosure. Separate from `expanded`: no note id keys it.
    let mut show_finished = use_signal(|| false);

    let outstanding =
        groups.read().iter().flat_map(|(_, _, rows)| rows).filter(|t| !t.done).count();
    let is_empty = groups.read().is_empty();

    // Cloned, not borrowed: props and handlers outlive any `read()` guard.
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
                        // Not an error: most notes hold no tasks, and proposing
                        // nothing is usually the correct outcome.
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
                    // Sibling of the groups (not a wrapper) so `.content`'s gap
                    // spaces revealed groups like the main list.
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

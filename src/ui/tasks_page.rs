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

use crate::notes::task::{Task, TaskStatus};
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

/// Accepted tasks, grouped by their note, newest note first.
///
/// Filtering happens **in here** rather than at the call site so the rule that
/// dismissed and undecided rows never reach this page is one testable place.
///
/// Done tasks sink to the bottom of their group rather than to a separate
/// section: the group is the note, and splitting a note's tasks across two
/// places would cost the provenance this page exists for. The sort is stable,
/// so ticking a box moves one row down and leaves everything else where the
/// eye last saw it.
fn group_accepted(tasks: &[Task]) -> Vec<(String, Vec<Task>)> {
    let mut rows: Vec<Task> =
        tasks.iter().filter(|t| t.status == TaskStatus::Accepted).cloned().collect();
    // Newest first, matching `TaskStore::accepted`. Group order then follows
    // first appearance, so the note you last accepted from sits at the top.
    rows.sort_by(|a, b| b.created.cmp(&a.created));

    let mut groups: Vec<(String, Vec<Task>)> = Vec::new();
    for task in rows {
        match groups.iter_mut().find(|(id, _)| *id == task.note_id) {
            Some((_, bucket)) => bucket.push(task),
            None => groups.push((task.note_id.clone(), vec![task])),
        }
    }
    for (_, bucket) in groups.iter_mut() {
        bucket.sort_by_key(|t| t.done);
    }
    groups
}

#[component]
pub fn TasksPage(props: TasksPageProps) -> Element {
    let notes = props.notes;
    let mut tasks = props.tasks;
    let registry = props.registry;

    // Cloned into owned rows rather than held as borrows: the controls below
    // capture their ids in click handlers, which cannot outlive a `read()`
    // guard on the store. Same reasoning as `notes_page`.
    let groups = use_memo(move || {
        let all = tasks.read().tasks.clone();
        let store = notes.read();
        group_accepted(&all)
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
mod tests {
    use super::*;
    use crate::notes::{NoteColor, NoteOrigin};

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
        }
    }

    fn texts(groups: &[(String, Vec<Task>)]) -> Vec<&str> {
        groups.iter().flat_map(|(_, rows)| rows).map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn dismissed_and_undecided_rows_never_reach_the_tasks_page() {
        let rows = vec![
            task("n1", "Call the vet", "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, false),
            task("n1", "Learn the piano", "2026-08-22T10:01:00+01:00", TaskStatus::Dismissed, false),
            task("n1", "Undecided", "2026-08-22T10:02:00+01:00", TaskStatus::Suggested, false),
        ];
        assert_eq!(
            texts(&group_accepted(&rows)),
            vec!["Call the vet"],
            "a dismissed row is a labelled negative kept for the eval corpus, not a task; \
             an undecided one is a proposal the user has not confirmed"
        );
    }

    #[test]
    fn tasks_are_grouped_under_the_note_that_produced_them() {
        let rows = vec![
            task("n1", "Call the vet", "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, false),
            task("n2", "Send the invoice", "2026-08-22T11:00:00+01:00", TaskStatus::Accepted, false),
            task("n1", "Book a table", "2026-08-22T10:30:00+01:00", TaskStatus::Accepted, false),
        ];
        let groups = group_accepted(&rows);
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
            texts(&group_accepted(&rows)),
            vec!["Still to do", "Done early"],
            "a ticked row must stop competing for attention with work that is left"
        );
    }

    #[test]
    fn a_ticked_task_stays_inside_its_own_note_group() {
        let rows = vec![
            task("n1", "Done", "2026-08-22T10:00:00+01:00", TaskStatus::Accepted, true),
            task("n2", "Open", "2026-08-22T11:00:00+01:00", TaskStatus::Accepted, false),
        ];
        let groups = group_accepted(&rows);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            texts(&groups[1..]),
            vec!["Done"],
            "done rows sink within their group, never into a separate section — the \
             group is the provenance this page exists to show"
        );
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
}

//! The two row-level components of the Tasks page.
//!
//! Split out of `tasks_page.rs` when the completed-task disclosure pushed that
//! file to 487 of its 500 allowed lines. The seam is presentational: everything
//! here renders one task, while the parent owns grouping, ordering and which
//! groups have their completed section open.
//!
//! `due_label` and `picked_due` stay in the parent deliberately. They are pure
//! and directly tested, and moving them here would drag the date suite along
//! with them for no gain.

use dioxus::prelude::*;

use chrono::NaiveDate;

use super::{due_label, picked_due};
use crate::notes::ics;
use crate::notes::task::Task;
use crate::notes::task_store::TaskStore;
use crate::ui::icons::IconCheck;

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
pub struct DueRowProps {
    pub task: Task,
    pub tasks: Signal<TaskStore>,
    pub today: NaiveDate,
}

#[component]
pub fn DueRow(props: DueRowProps) -> Element {
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

/// One task: its checkbox, its text, its date row and the words it came from.
///
/// A component rather than inline markup because the page renders rows in two
/// places — outstanding, and inside the completed disclosure — and a row
/// duplicated across both is a row that will drift between them.
#[derive(Props, Clone, PartialEq)]
pub struct TaskRowProps {
    pub task: Task,
    pub tasks: Signal<TaskStore>,
    pub today: NaiveDate,
}

#[component]
pub fn TaskRow(props: TaskRowProps) -> Element {
    let TaskRowProps { task, mut tasks, today } = props;
    let id = task.id.clone();
    let done = task.done;

    rsx! {
        div {
            class: if done { "task-row done" } else { "task-row" },
            button {
                class: if done { "task-check checked" } else { "task-check" },
                title: if done { "Mark as not done" } else { "Mark as done" },
                onclick: move |_| { tasks.write().set_done(&id, !done); },
                if done {
                    IconCheck { size: 12 }
                }
            }
            div { class: "task-row-main",
                div { class: "task-text", "{task.text}" }
                DueRow { task: task.clone(), tasks, today }
                // Full span on hover: it is one ellipsized line, and being able
                // to check it is the whole point.
                div { class: "task-evidence", title: "{task.evidence}",
                    "\u{201c}{task.evidence}\u{201d}"
                }
            }
        }
    }
}

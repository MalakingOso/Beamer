//! Row-level components of the Tasks page: everything here renders one task,
//! while the parent owns grouping, ordering and disclosure state.
//!
//! `due_label` and `picked_due` stay in the parent: they are pure and directly
//! tested, and moving them would drag the date suite along for no gain.

use dioxus::prelude::*;

use chrono::NaiveDate;

use super::{due_label, picked_due};
use crate::notes::ics;
use crate::notes::task::Task;
use crate::notes::task_store::TaskStore;
use crate::ui::icons::IconCheck;

/// The dated half of a task row: a chip when resolved, the model's raw phrase
/// beside a picker when not, nothing when no timing was mentioned. Calendar
/// export is always an explicit per-task click, never automatic.
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
                        // One-way: the calendar imports a copy Beamer cannot edit — see `notes::ics`.
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
            // Shown rather than swallowed: says what to fill the picker in with.
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

/// One task: checkbox, text, date row and the words it came from. A component
/// because rows render in two places (outstanding and completed disclosure).
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
                // One ellipsized line; the full span is on hover via `title`.
                div { class: "task-evidence", title: "{task.evidence}",
                    "\u{201c}{task.evidence}\u{201d}"
                }
            }
        }
    }
}

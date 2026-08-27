//! One note's box of tasks, heading and all.
//!
//! Extracted from `tasks_page.rs` when finished notes gained a page-level
//! Completed section: the box now renders in two places, and the same reasoning
//! that pulled `TaskRow` out of the page applies here — markup duplicated across
//! two regions is markup that will drift between them.
//!
//! Scoped `pub(super)` rather than `pub`. A child module can already see its
//! parent's private items, so `Heading` is reachable here without widening it,
//! and it is used nowhere else in `src/`. The only reason the scope is stated at
//! all is that a `pub` props struct may not expose a private field type.

use std::collections::HashSet;

use dioxus::prelude::*;

use chrono::NaiveDate;

use super::rows::TaskRow;
use super::{split_done, Heading};
use crate::notes::task::Task;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::ui::sticky_windows::{self, StickyRegistry};

#[derive(Props, Clone, PartialEq)]
pub(super) struct TaskGroupProps {
    pub note_id: String,
    pub heading: Heading,
    pub rows: Vec<Task>,
    pub tasks: Signal<TaskStore>,
    pub notes: Signal<NoteStore>,
    pub registry: StickyRegistry,
    pub today: NaiveDate,
    /// Every task here is done. Rows render directly with no inner
    /// disclosure — a caret offering to reveal "the completed ones" inside a
    /// group that is entirely completed is a control with nothing to hide.
    pub finished: bool,
    /// Which unfinished groups have their inner disclosure open, keyed by
    /// note id. Ignored when `finished`.
    pub expanded: Signal<HashSet<String>>,
}

#[component]
pub(super) fn TaskGroup(props: TaskGroupProps) -> Element {
    let TaskGroupProps {
        note_id,
        heading,
        rows,
        tasks,
        notes,
        registry,
        today,
        finished,
        mut expanded,
    } = props;

    let (outstanding_rows, done_rows) = split_done(&rows);
    let open = expanded.read().contains(&note_id);
    let count = done_rows.len();

    rsx! {
        div { class: "task-group",
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
            if finished {
                // Every row is in the done half, so there is no outstanding
                // half to draw and nothing for a disclosure to hide. Rendering
                // the split's completed half directly is what keeps this
                // branch honest: the rows come from the same function either
                // way, so a group cannot lose one by taking this path.
                for task in done_rows.iter() {
                    TaskRow { key: "{task.id}", task: (*task).clone(), tasks, today }
                }
            } else {
                for task in outstanding_rows.iter() {
                    TaskRow { key: "{task.id}", task: (*task).clone(), tasks, today }
                }
                if count > 0 {
                    // The count is load-bearing, not decoration. Ticking a box
                    // now makes a row vanish rather than sink, and this number
                    // ticking up is the only thing that says where it went.
                    button {
                        class: "task-done-toggle",
                        title: if open { "Hide completed tasks" } else { "Show completed tasks" },
                        onclick: {
                            let id = note_id.clone();
                            move |_| {
                                let mut open_ids = expanded.write();
                                if !open_ids.remove(&id) {
                                    open_ids.insert(id.clone());
                                }
                            }
                        },
                        span { class: "task-done-chevron",
                            if open { "\u{25BE}" } else { "\u{25B8}" }
                        }
                        "{count} completed"
                    }
                    if open {
                        for task in done_rows.iter() {
                            TaskRow { key: "{task.id}", task: (*task).clone(), tasks, today }
                        }
                    }
                }
            }
        }
    }
}

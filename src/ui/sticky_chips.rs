//! Suggestion chips on a sticky note.
//!
//! A chip is a *proposal*, never a fact. Extraction writes nothing to the
//! Tasks page directly: it puts undecided rows here, and one click each way
//! decides them. That confirmation step is the feature's actual precision
//! mechanism — it makes a false positive cost one click instead of quietly
//! polluting a task list, and it is why a 5 GB model is a reasonable default.
//!
//! The store arrives as a **prop**, like `notes`, for the reason documented at
//! the top of `sticky.rs`: each sticky is its own `VirtualDom`, so
//! `use_context` cannot see the main window's providers and a `GlobalSignal`
//! would silently give every window its own copy.
//!
//! Chips render **outside `.sticky-bar`**. Every control inside the bar needs
//! `onmousedown: e.stop_propagation()` or the bar's window drag swallows the
//! click; out here that is not needed and its absence is not a bug.
//!
//! The reported confidence is deliberately **not** shown. Measured against the
//! real model it clusters between 0.90 and 0.98 whatever the note, so a number
//! on the chip would look like information and carry none. The evidence span is
//! the honest signal: it is what makes a wrong suggestion obvious at a glance.

use dioxus::prelude::*;

use crate::notes::task::Task;
use crate::notes::task_store::TaskStore;
use crate::ui::icons::{IconCheck, IconX};

#[derive(Props, Clone, PartialEq)]
pub struct StickyChipsProps {
    pub note_id: String,
    pub tasks: Signal<TaskStore>,
}

#[component]
pub fn StickyChips(props: StickyChipsProps) -> Element {
    let StickyChipsProps { note_id, mut tasks } = props;

    // Cloned into owned rows rather than held as borrows: the buttons below
    // capture their ids in click handlers, which cannot outlive a `read()`
    // guard on the store. Same reasoning as `notes_page`.
    let rows = {
        let note_id = note_id.clone();
        use_memo(move || {
            tasks
                .read()
                .suggested_for(&note_id)
                .into_iter()
                .cloned()
                .collect::<Vec<Task>>()
        })
    };

    if rows.read().is_empty() {
        // No strip at all rather than an empty one. Most notes contain no
        // tasks, so an "as yet nothing" row would be the common case and would
        // spend a note's height saying nothing.
        return rsx! {};
    }

    rsx! {
        div { class: "sticky-chips",
            for task in rows.read().iter() {
                {
                    let id = task.id.clone();
                    let text = task.text.clone();
                    let evidence = task.evidence.clone();
                    rsx! {
                        div { key: "{id}", class: "sticky-chip",
                            div { class: "sticky-chip-main",
                                div { class: "sticky-chip-text", "{text}" }
                                // Full text on hover: the span is one line and
                                // ellipsized, and the whole point of showing it
                                // is that the user can check it.
                                div {
                                    class: "sticky-chip-evidence",
                                    title: "{evidence}",
                                    "\u{201c}{evidence}\u{201d}"
                                }
                            }
                            div { class: "sticky-chip-actions",
                                button {
                                    class: "sticky-chip-btn sticky-chip-accept",
                                    title: "Add to tasks",
                                    onclick: {
                                        let id = id.clone();
                                        move |_| { tasks.write().accept(&id); }
                                    },
                                    IconCheck { size: 14 }
                                }
                                button {
                                    class: "sticky-chip-btn sticky-chip-dismiss",
                                    // Not "delete": the row is kept. Dismissing
                                    // is how the model is told it was wrong, and
                                    // that answer is the labelled negative the
                                    // eval corpus is built from.
                                    title: "Not a task",
                                    onclick: {
                                        let id = id.clone();
                                        move |_| { tasks.write().dismiss(&id); }
                                    },
                                    IconX { size: 13 }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

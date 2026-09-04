//! Suggestion chips: extraction proposes, one click each way decides (a false
//! positive costs one click, not a polluted task list). The store arrives as a
//! prop — each sticky is its own `VirtualDom`, so `use_context` can't see the
//! main window's providers. Chips render outside `.sticky-bar`, so no
//! `stop_propagation` dance is needed. Confidence is not shown (it clusters
//! 0.90–0.98 regardless); the evidence span is the signal.

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

    // Cloned: click handlers can't hold a `read()` guard on the store.
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
        // No strip at all rather than an empty placeholder row.
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
                                // Full text on hover; the span is ellipsized to one line.
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
                                    // Kept as a labelled negative for the eval corpus.
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

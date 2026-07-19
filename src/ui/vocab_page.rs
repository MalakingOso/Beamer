use dioxus::prelude::*;

use crate::ui::icons::{IconPencil, IconTrash};

#[component]
pub fn VocabPage() -> Element {
    let mut vocab_terms = use_signal(|| {
        crate::config::vocabulary::Vocabulary::load()
            .map(|v| v.list().to_vec())
            .unwrap_or_default()
    });
    let mut new_term = use_signal(|| String::new());
    let mut filter = use_signal(|| String::new());
    // Which term index is being edited (None = not editing)
    let mut editing: Signal<Option<usize>> = use_signal(|| None);
    let mut edit_value = use_signal(|| String::new());

    let filtered = use_memo(move || {
        let query = filter.read().to_lowercase();
        vocab_terms
            .read()
            .iter()
            .enumerate()
            .filter(|(_, t)| query.is_empty() || t.to_lowercase().contains(&query))
            .map(|(i, t)| (i, t.clone()))
            .collect::<Vec<(usize, String)>>()
    });

    let total_count = vocab_terms.read().len();
    let shown_count = filtered.read().len();

    let mut add_term = move || {
        let term = new_term.read().trim().to_string();
        if term.is_empty() {
            return;
        }
        let mut terms = vocab_terms.read().clone();
        if !terms.contains(&term) {
            terms.push(term.clone());
            vocab_terms.set(terms);
        }
        if let Ok(mut vocab) = crate::config::vocabulary::Vocabulary::load() {
            if let Err(e) = vocab.add(&term) {
                tracing::error!("Failed to save vocabulary term: {}", e);
            }
        }
        new_term.set(String::new());
    };

    let mut remove_term = move |term: String| {
        let mut terms = vocab_terms.read().clone();
        terms.retain(|t| t != &term);
        vocab_terms.set(terms);
        if let Ok(mut vocab) = crate::config::vocabulary::Vocabulary::load() {
            if let Err(e) = vocab.remove(&term) {
                tracing::error!("Failed to remove vocabulary term: {}", e);
            }
        }
    };

    let mut save_edit = move |idx: usize| {
        let new_val = edit_value.read().trim().to_string();
        if new_val.is_empty() {
            editing.set(None);
            return;
        }
        let mut terms = vocab_terms.read().clone();
        if idx < terms.len() {
            let old = terms[idx].clone();
            if old != new_val {
                // Remove old, add new on disk
                if let Ok(mut vocab) = crate::config::vocabulary::Vocabulary::load() {
                    let _ = vocab.remove(&old);
                    let _ = vocab.add(&new_val);
                }
                terms[idx] = new_val;
                vocab_terms.set(terms);
            }
        }
        editing.set(None);
    };

    rsx! {
        div { class: "content",
            div { class: "vocab-header",
                h1 { class: "vocab-title", "Vocabulary" }
                {
                    let label = if total_count == 0 {
                        "No terms".to_string()
                    } else if shown_count == total_count {
                        if total_count == 1 { "1 term".to_string() } else { format!("{total_count} terms") }
                    } else {
                        format!("{shown_count} of {total_count}")
                    };
                    rsx! { span { class: "vocab-count", "{label}" } }
                }
            }

            p { class: "vocab-description",
                "Custom terms help the transcription engine recognise names, acronyms, and jargon it wouldn't otherwise get right."
            }

            // Add new term
            div { class: "vocab-add-row",
                input {
                    class: "input input-mono vocab-add-input",
                    placeholder: "Add a term\u{2026}",
                    value: "{new_term}",
                    oninput: move |e: Event<FormData>| {
                        new_term.set(e.value().to_string());
                    },
                    onkeypress: move |e: Event<KeyboardData>| {
                        if e.key() == Key::Enter {
                            add_term();
                        }
                    },
                }
                button {
                    class: "btn btn-primary btn-small",
                    disabled: new_term.read().trim().is_empty(),
                    onclick: move |_| add_term(),
                    "Add"
                }
            }

            // Filter
            if total_count > 5 {
                input {
                    class: "input vocab-filter",
                    placeholder: "Filter terms\u{2026}",
                    value: "{filter}",
                    oninput: move |e: Event<FormData>| {
                        filter.set(e.value().to_string());
                    },
                }
            }

            // Term list
            if filtered.read().is_empty() && total_count > 0 {
                div { class: "empty-state",
                    span { class: "empty-state-text", "No matching terms" }
                }
            } else if filtered.read().is_empty() {
                div { class: "empty-state",
                    span { class: "empty-state-text", "No vocabulary terms yet" }
                    span { class: "empty-state-hint", "Add terms above to improve transcription accuracy." }
                }
            } else {
                for (idx, term) in filtered.read().iter() {
                    {
                        let idx = *idx;
                        let term = term.clone();
                        let is_editing = *editing.read() == Some(idx);
                        rsx! {
                            div {
                                class: if is_editing { "vocab-term-card editing" } else { "vocab-term-card" },
                                key: "{idx}",
                                if is_editing {
                                    input {
                                        class: "input input-mono vocab-edit-input",
                                        value: "{edit_value}",
                                        oninput: move |e: Event<FormData>| {
                                            edit_value.set(e.value().to_string());
                                        },
                                        onkeypress: move |e: Event<KeyboardData>| {
                                            if e.key() == Key::Enter {
                                                save_edit(idx);
                                            } else if e.key() == Key::Escape {
                                                editing.set(None);
                                            }
                                        },
                                        autofocus: true,
                                    }
                                    button {
                                        class: "btn btn-small",
                                        onclick: move |_| save_edit(idx),
                                        "Save"
                                    }
                                    button {
                                        class: "btn btn-small",
                                        onclick: move |_| editing.set(None),
                                        "Cancel"
                                    }
                                } else {
                                    span { class: "vocab-row-text", "{term}" }
                                    div { class: "vocab-row-actions",
                                        button {
                                            class: "vocab-action-btn",
                                            onclick: {
                                                let term = term.clone();
                                                move |_| {
                                                    edit_value.set(term.clone());
                                                    editing.set(Some(idx));
                                                }
                                            },
                                            IconPencil { size: 15 }
                                        }
                                        button {
                                            class: "vocab-action-btn danger",
                                            onclick: {
                                                let term = term.clone();
                                                move |_| remove_term(term.clone())
                                            },
                                            IconTrash { size: 15 }
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



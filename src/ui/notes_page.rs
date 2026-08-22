//! The notes board — every note in one place, searchable.
//!
//! Sticky windows are how you *use* a note; this is how you find one again once
//! there are more than a handful, and how you get back a note whose window you
//! closed. Clicking a card is the only way back to a closed note, which is why
//! `sticky_windows::reopen_note` prunes stale registry slots at this gesture
//! rather than in the reconciler.
//!
//! The store and the window registry arrive as **props**. Nothing in `src/`
//! calls `provide_context`, so `use_context` here would panic rather than
//! resolve; pages in this app take props.

use dioxus::prelude::*;

use crate::notes::{Note, NoteStore};
use crate::ui::components::Toggle;
use crate::ui::sticky_windows::{self, StickyRegistry};

#[derive(Props, Clone, PartialEq)]
pub struct NotesPageProps {
    pub notes: Signal<NoteStore>,
    pub registry: StickyRegistry,
}

/// How much of a note's body a card shows before trimming.
const PREVIEW_CHARS: usize = 180;

fn preview(note: &Note) -> String {
    let flat = note.body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(PREVIEW_CHARS).collect();
    format!("{}\u{2026}", cut.trim_end())
}

fn when(note: &Note) -> String {
    chrono::DateTime::parse_from_rfc3339(&note.created)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%d %b, %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| "unknown".to_string())
}

#[component]
pub fn NotesPage(props: NotesPageProps) -> Element {
    let mut notes = props.notes;
    let registry = props.registry;

    let mut query = use_signal(String::new);
    let mut show_archived = use_signal(|| false);

    // Cloned into owned `Note`s rather than held as borrows: the rows below
    // capture their ids in click handlers, which cannot outlive a `read()`
    // guard on the store.
    let visible = use_memo(move || {
        let store = notes.read();
        let rows: Vec<&Note> = if *show_archived.read() {
            store.archived()
        } else {
            store.search(&query.read())
        };
        rows.into_iter().cloned().collect::<Vec<Note>>()
    });

    let active_total = use_memo(move || notes.read().active().len());
    let archived_total = use_memo(move || notes.read().archived().len());

    let showing_archived = *show_archived.read();
    let shown = visible.read().len();

    rsx! {
        div { class: "content",
            div { class: "notes-header",
                h1 { class: "notes-title", "Notes" }
                {
                    let label = if showing_archived {
                        format!("{shown} archived")
                    } else if shown == active_total() {
                        match shown {
                            0 => "No notes".to_string(),
                            1 => "1 note".to_string(),
                            n => format!("{n} notes"),
                        }
                    } else {
                        format!("{shown} of {}", active_total())
                    };
                    rsx! { span { class: "notes-count", "{label}" } }
                }
            }

            p { class: "notes-description",
                "Dictate into a note with the note hotkey. Search covers what you actually said, not just the cleaned-up text."
            }

            if !showing_archived {
                input {
                    class: "input notes-search",
                    placeholder: "Search notes\u{2026}",
                    value: "{query}",
                    oninput: move |e: Event<FormData>| query.set(e.value().to_string()),
                }
            }

            div { class: "notes-archived-row",
                span { class: "notes-archived-label",
                    "Show archived ({archived_total})"
                }
                Toggle {
                    value: showing_archived,
                    ontoggle: move |v: bool| show_archived.set(v),
                }
            }

            if visible.read().is_empty() {
                div { class: "empty-state",
                    if showing_archived {
                        span { class: "empty-state-text", "Nothing archived" }
                    } else if query.read().trim().is_empty() {
                        span { class: "empty-state-text", "No notes yet" }
                        span { class: "empty-state-hint",
                            "Set a note hotkey in Settings, then hold it and speak."
                        }
                    } else {
                        span { class: "empty-state-text", "No matching notes" }
                    }
                }
            } else {
                for note in visible.read().iter() {
                    {
                        let id = note.id.clone();
                        let stripe = note.color.css_class();
                        let text = preview(note);
                        let stamp = when(note);
                        let archived = note.archived;
                        rsx! {
                            div {
                                key: "{id}",
                                class: if archived { "note-card archived" } else { "note-card" },
                                title: if archived { "" } else { "Open this note" },
                                onclick: {
                                    let id = id.clone();
                                    move |_| {
                                        if !archived {
                                            sticky_windows::reopen_note(registry, notes, &id);
                                        }
                                    }
                                },
                                div { class: "note-stripe note-stripe-{stripe}" }
                                div { class: "note-card-main",
                                    div { class: "note-card-body", "{text}" }
                                    div { class: "note-card-meta", "{stamp}" }
                                }
                                div { class: "note-card-actions",
                                    if archived {
                                        button {
                                            class: "note-action-btn",
                                            onclick: {
                                                let id = id.clone();
                                                move |e: Event<MouseData>| {
                                                    e.stop_propagation();
                                                    notes.write().restore(&id);
                                                }
                                            },
                                            "Restore"
                                        }
                                    } else {
                                        button {
                                            class: "note-action-btn",
                                            onclick: {
                                                let id = id.clone();
                                                move |e: Event<MouseData>| {
                                                    // Without this the card's own
                                                    // handler also fires and reopens
                                                    // the note we just archived.
                                                    e.stop_propagation();
                                                    notes.write().archive(&id);
                                                }
                                            },
                                            "Archive"
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

    fn note_with(body: &str) -> Note {
        let mut store = NoteStore::default();
        let id = store.create(body.to_string(), NoteColor::Purple, NoteOrigin::Dictated);
        store.get(&id).unwrap().clone()
    }

    #[test]
    fn preview_collapses_the_whitespace_a_transcript_carries() {
        let n = note_with("call   the\n\nvet  ");
        assert_eq!(preview(&n), "call the vet");
    }

    #[test]
    fn preview_trims_a_long_note_and_marks_it() {
        let n = note_with(&"word ".repeat(100));
        let p = preview(&n);
        assert!(p.ends_with('\u{2026}'), "a trimmed preview must say so: {p}");
        assert!(p.chars().count() <= PREVIEW_CHARS + 1);
    }

    #[test]
    fn a_short_note_is_shown_whole() {
        let n = note_with("short one");
        assert_eq!(preview(&n), "short one");
    }
}

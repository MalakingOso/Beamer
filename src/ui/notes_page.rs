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

use crate::config::Config;
use crate::notes::blocks;
use crate::notes::task_store::TaskStore;
use crate::notes::{Note, NoteColor, NoteOrigin, NoteStore};
use crate::ui::components::Toggle;
use crate::ui::icons::IconPlus;
use crate::ui::sticky_windows::{self, StickyRegistry};

#[derive(Props, Clone, PartialEq)]
pub struct NotesPageProps {
    pub notes: Signal<NoteStore>,
    /// Read only for `notes.default_color`, so a typed note is born the same
    /// colour a dictated one would be.
    pub config: Signal<Config>,
    /// Needed only by Delete: a note removed outright takes its rows with it.
    pub tasks: Signal<TaskStore>,
    pub registry: StickyRegistry,
}

/// How much of a note's body a card shows before trimming.
const PREVIEW_CHARS: usize = 180;

/// A card's text, with attachment tokens stripped.
///
/// Without `plain_text` a card for a note holding a photo would read
/// "[[beamer:18f2a…]]", which is both meaningless and the only thing a
/// picture-only note would show.
fn preview(note: &Note) -> String {
    let body = blocks::plain_text(&note.body);
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
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
    let config = props.config;
    let mut tasks = props.tasks;
    let registry = props.registry;

    let mut query = use_signal(String::new);
    let mut show_archived = use_signal(|| false);
    // Which archived note has been asked about but not yet confirmed. A
    // two-step button rather than a modal: deletion is permanent and needs a
    // deliberate second act, and this app has no dialog primitive to borrow.
    let mut pending_delete: Signal<Option<String>> = use_signal(|| None);

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
                div { class: "notes-header-actions",
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
                    button {
                        class: "notes-new-btn",
                        title: "New note",
                        onclick: move |_| {
                            // No pipeline request, deliberately. The automatic
                            // trigger lives at exactly one site —
                            // `do_note_capture`, reachable only from dictation
                            // — which is what makes "a typed note is never
                            // rewritten unasked" structural rather than a check
                            // somebody could forget. S1-mini normalizes
                            // *transcripts*; typed prose is outside its
                            // training distribution, quite apart from it being
                            // presumptuous to rewrite what someone deliberately
                            // wrote. A typed note reaches a model only when the
                            // user presses the note's own footer affordance.
                            //
                            // No `new_window` call either: `create` sets
                            // `open: true`, and the reconciler opens a window
                            // for any note that is open and not archived.
                            let color =
                                NoteColor::from_config_name(&config.peek().notes.default_color);
                            notes.write().create(String::new(), color, NoteOrigin::Typed);
                            // Flushed inline, like `do_note_capture`, and
                            // through `flush_stores` because that is the only
                            // thing that writes the document. `flush_if_dirty`
                            // would write the JSON mirror alone, which nothing
                            // reads back.
                            crate::notes::flush_stores(
                                &mut notes.write(),
                                &mut tasks.write(),
                            );
                        },
                        IconPlus { size: 14 }
                    }
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
                        let clips = note.attachments.len();
                        let confirming = pending_delete.read().as_deref() == Some(id.as_str());
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
                                    div { class: "note-card-meta",
                                        "{stamp}"
                                        if clips > 0 {
                                            span { class: "note-card-clips",
                                                title: if clips == 1 { "1 attachment" } else { "attachments" },
                                                "\u{1F4CE} {clips}"
                                            }
                                        }
                                    }
                                }
                                div { class: "note-card-actions",
                                    if archived {
                                        button {
                                            class: "note-action-btn",
                                            onclick: {
                                                let id = id.clone();
                                                move |e: Event<MouseData>| {
                                                    e.stop_propagation();
                                                    pending_delete.set(None);
                                                    notes.write().restore(&id);
                                                }
                                            },
                                            "Restore"
                                        }
                                        // Delete is offered only on an archived
                                        // note, and only in two steps. Archive
                                        // stays the everyday gesture; this is
                                        // the one that cannot be undone, and it
                                        // takes the note's task rows with it.
                                        button {
                                            class: if confirming {
                                                "note-action-btn note-action-danger"
                                            } else {
                                                "note-action-btn"
                                            },
                                            title: if confirming {
                                                "Deletes the note and its tasks. Your files are never touched."
                                            } else {
                                                "Delete permanently"
                                            },
                                            onclick: {
                                                let id = id.clone();
                                                move |e: Event<MouseData>| {
                                                    e.stop_propagation();
                                                    if !confirming {
                                                        pending_delete.set(Some(id.clone()));
                                                        return;
                                                    }
                                                    pending_delete.set(None);
                                                    // Rows first: a crash
                                                    // between the two would
                                                    // otherwise strand tasks
                                                    // whose note is gone,
                                                    // rather than a note whose
                                                    // rows are.
                                                    tasks.write().delete_for_note(&id);
                                                    notes.write().delete(&id);
                                                    crate::notes::flush_stores(
                                                        &mut notes.write(),
                                                        &mut tasks.write(),
                                                    );
                                                }
                                            },
                                            if confirming { "Really delete?" } else { "Delete" }
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

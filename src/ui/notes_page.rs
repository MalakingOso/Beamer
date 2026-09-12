//! The notes board — every note in one place, searchable. Clicking a card is
//! the way back to a closed note's window.
//!
//! Store and registry arrive as props: nothing in `src/` calls
//! `provide_context`, so `use_context` here would panic rather than resolve.

use dioxus::prelude::*;

use crate::notes::task_store::TaskStore;
use crate::notes::{Note, NoteColor, NoteOrigin, NoteStore};
use crate::ui::components::Toggle;
use crate::ui::icons::IconPlus;
use crate::ui::sticky_windows::{self, StickyRegistry};

#[derive(Props, Clone, PartialEq)]
pub struct NotesPageProps {
    pub notes: Signal<NoteStore>,
    /// Needed only by Delete: a note removed outright takes its rows with it.
    pub tasks: Signal<TaskStore>,
    pub registry: StickyRegistry,
}

/// How much of a note's body a card shows before trimming.
const PREVIEW_CHARS: usize = 180;

/// Card text, whitespace-collapsed.
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
    let mut tasks = props.tasks;
    let registry = props.registry;

    let mut query = use_signal(String::new);
    let mut show_archived = use_signal(|| false);
    // Two-step delete (no dialog primitive): which archived note awaits confirmation.
    let mut pending_delete: Signal<Option<String>> = use_signal(|| None);

    // Cloned to owned `Note`s: row click handlers cannot outlive a `read()` guard.
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
    // Drives the Hide-all/Show-all toggle: any open window means "hide".
    let open_count = use_memo(move || {
        let store = notes.read();
        store.active().iter().filter(|n| store.is_open(&n.id)).count()
    });

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
                    // Bulk window control. Not a true minimize: notes skip the
                    // taskbar on Windows and are undecorated everywhere, so a
                    // minimized window would be unrecoverable except from here.
                    // Closing through `set_all_open` keeps every note one board
                    // click away instead. Hidden in the archived view, which it
                    // does not act on.
                    if !showing_archived && active_total() > 0 {
                        if open_count() > 0 {
                            button {
                                class: "note-action-btn",
                                title: "Close all note windows — the notes stay on the board",
                                onclick: move |_| {
                                    notes.write().set_all_open(false);
                                },
                                "Hide all"
                            }
                        } else {
                            button {
                                class: "note-action-btn",
                                title: "Reopen every note",
                                onclick: move |_| {
                                    notes.write().set_all_open(true);
                                },
                                "Show all"
                            }
                        }
                    }
                    button {
                        class: "notes-new-btn",
                        title: "New note",
                        onclick: move |_| {
                            // No pipeline request: typed notes reach a model only
                            // via the note's own footer affordance, never unasked.
                            // No `new_window` call: `create` sets `open: true`,
                            // which is what the reconciler opens.
                            notes.write().create(
                                String::new(),
                                NoteColor::random(),
                                NoteOrigin::Typed,
                            );
                            // Inline through `flush_stores` (the only document
                            // writer), like `do_note_capture`.
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
                                        // Archived notes only, two steps: permanent,
                                        // and takes the note's task rows with it.
                                        button {
                                            class: if confirming {
                                                "note-action-btn note-action-danger"
                                            } else {
                                                "note-action-btn"
                                            },
                                            title: if confirming {
                                                "Deletes the note and its tasks."
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
                                                    // Rows first, so a crash can't strand
                                                    // tasks whose note is gone.
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
                                                    // Else the card reopens the note just archived.
                                                    e.stop_propagation();
                                                    notes.write().archive(&id);
                                                    // Flush inline like delete: a crash inside
                                                    // the tick window must not resurrect it.
                                                    crate::notes::flush_stores(
                                                        &mut notes.write(),
                                                        &mut tasks.write(),
                                                    );
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

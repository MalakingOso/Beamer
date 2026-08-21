//! Opening, closing and reconciling sticky note windows.
//!
//! Kept out of `app_setup.rs`, which is already the largest hook file and gains
//! more in Task 10 (window placement). Nothing here is a Dioxus component — it
//! is the bridge between the note store and the desktop windowing layer.
//!
//! **Why a reconciler rather than an open-on-create call.** Windows are not
//! opened by whoever creates a note. Instead one effect watches the store and
//! makes the set of live windows match the set of notes that should be showing:
//!
//! ```text
//! open && !archived && not registered  ->  open a window
//! (archived || !open) && registered    ->  close it
//! ```
//!
//! One mechanism therefore covers both a note dictated just now and a note
//! loaded from `notes.json` at startup. An open-on-create call would restore
//! the file on relaunch but leave the desktop empty.

use std::collections::HashMap;

use dioxus::desktop::tao::dpi::{LogicalPosition, LogicalSize};
use dioxus::desktop::{Config as DesktopConfig, DesktopContext, WindowBuilder};
use dioxus::prelude::*;

use crate::notes::{Note, NoteStore};
use crate::ui::sticky::{window_title, StickyNote, StickyNoteProps, STICKY_CSS};

/// Maps note id -> its live window, so a note cannot be opened twice and can be
/// closed programmatically when archived.
///
/// The value is `Option<DesktopContext>` because `new_window` is async: the slot
/// is reserved with `None` *before* awaiting, so a second reconcile pass during
/// the await sees the id as taken. Without the reservation a note gets two
/// windows.
pub type StickyRegistry = Signal<HashMap<String, Option<DesktopContext>>>;

/// Open one note's window. Assumes the caller has already reserved `note.id` in
/// the registry.
async fn open_note_window(
    window: DesktopContext,
    mut registry: StickyRegistry,
    notes: Signal<NoteStore>,
    note: Note,
) {
    let (w, h) = note.size.unwrap_or((320, 260));
    let mut builder = WindowBuilder::new()
        .with_title(window_title(&note.id))
        .with_decorations(false)
        .with_always_on_top(false)
        .with_inner_size(LogicalSize::new(w as f64, h as f64));

    // Honored on Windows; ignored by Mutter, which is why the GNOME extension
    // exists. Set anyway so the Windows build needs no special case.
    if let Some((x, y)) = note.pos {
        builder = builder.with_position(LogicalPosition::new(x as f64, y as f64));
    }

    let cfg = DesktopConfig::new()
        .with_data_directory(super::webview_data_dir())
        .with_window(builder)
        .with_custom_head(format!("<style>{STICKY_CSS}</style>"))
        // Load-bearing for a tray app: without it, archiving the last sticky
        // while the main window is hidden would exit Beamer entirely.
        .with_exits_when_last_window_closes(false);

    let dom = VirtualDom::new_with_props(
        StickyNote,
        StickyNoteProps { id: note.id.clone(), notes },
    );

    let ctx: DesktopContext = window.new_window(dom, cfg).await;
    registry.write().insert(note.id.clone(), Some(ctx));
}

/// Close a note's window and drop its registry slot.
pub fn close_note_window(mut registry: StickyRegistry, id: &str) {
    if let Some(Some(ctx)) = registry.write().remove(id) {
        ctx.close();
    }
}

/// Watch the note store and keep the set of open windows matching it.
///
/// Call once from `App()`.
pub fn setup_sticky_windows(window: DesktopContext, notes: Signal<NoteStore>) {
    let registry: StickyRegistry = use_signal(HashMap::new);

    use_effect(move || {
        // Subscribe to the store — this is the trigger. The registry is only
        // ever `peek`ed, so opening a window does not re-run this effect.
        let store = notes.read();

        let mut to_open: Vec<Note> = Vec::new();
        let mut to_close: Vec<String> = Vec::new();
        {
            let live = registry.peek();
            for note in store.notes.iter() {
                let showing = note.open && !note.archived;
                let registered = live.contains_key(&note.id);
                if showing && !registered {
                    to_open.push(note.clone());
                } else if !showing && registered {
                    to_close.push(note.id.clone());
                }
            }
        }
        drop(store);

        for id in to_close {
            close_note_window(registry, &id);
        }

        for note in to_open {
            // Reserve the slot synchronously, before the await inside
            // `open_note_window`, so a reconcile pass that lands mid-open does
            // not open the same note twice.
            let mut reg = registry;
            if reg.write().insert(note.id.clone(), None).is_some() {
                continue;
            }
            let window = window.clone();
            spawn(async move {
                open_note_window(window, reg, notes, note).await;
            });
        }
    });
}

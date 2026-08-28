//! Sticky note windows.
//!
//! One ordinary Dioxus window per open note — the same `new_window` pattern
//! used for the splash and pill windows in `app_setup.rs`. Deliberately NOT
//! always-on-top: notes sit in the normal stacking order.
//!
//! Wayland gives clients no control over their own position, so placement goes
//! through Beamer's GNOME extension (`shell_window`); the window title is the
//! handle it matches on. There is no position read-back — notes are placed by
//! `note_layout`, never restored to where they were.
//!
//! **Size is different, and is remembered.** Unlike position, a window's size
//! arrives in the compositor's configure event rather than having to be
//! guessed, so `WindowEvent::Resized` is a fact rather than a hopeful read.
//! That does not reopen the scatter-on-launch decision: a note still appears
//! somewhere new each launch, now at the size you left it.
//!
//! The store arrives as a **prop**, not via `use_context`. Each sticky window is
//! its own `VirtualDom` with its own scope tree, and `use_context` walks only
//! the current dom's tree — the main window's provider is invisible from here.
//! A `Signal` is `Copy + 'static` and its generational-box arena is thread-local
//! (`generational-box`'s `UNSYNC_RUNTIME`), and every desktop `VirtualDom` polls
//! on the main thread, so the handle resolves across the dom boundary even
//! though the context does not.

// Aliased: `dioxus::prelude::Event` is the UI event type used all through
// the component below, and tao's is a different thing entirely.
use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
use dioxus::desktop::tao::window::ResizeDirection;
use dioxus::desktop::{use_window, use_wry_event_handler};
use dioxus::html::HasFileData;
use dioxus::prelude::*;

use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::{next_id, Attachment, NoteColor, NoteStore};
use crate::ui::icons::{IconAsterisk, IconCheck};
use crate::ui::sticky_blocks::{self, StickyBody};
use crate::ui::sticky_chips::StickyChips;
use crate::ui::sticky_footer::{self, FooterIcon};

pub const TITLE_PREFIX: &str = "Beamer Note ";

pub fn window_title(id: &str) -> String {
    format!("{TITLE_PREFIX}{id}")
}

/// Every colour a note can be, in swatch order.
pub const PALETTE: [NoteColor; 6] = [
    NoteColor::Purple,
    NoteColor::Violet,
    NoteColor::Amber,
    NoteColor::Teal,
    NoteColor::Rose,
    NoteColor::Slate,
];

/// Convert a `Resized` event's physical size to the logical one the store keeps.
///
/// ⚠️ **`Resized` carries `PhysicalSize`; the window was built from a
/// `LogicalSize`.** At scale 1.0 the two are identical, so getting this
/// backwards is completely invisible on a 1x display and reopens every note at
/// double or half size on a HiDPI one — the same trap `work_area` documents for
/// monitor geometry.
///
/// A zero in either axis is a minimize on some compositors, not a resize, and
/// storing it would reopen the note as a sliver.
pub fn logical_size(physical: (u32, u32), scale: f64) -> Option<(u32, u32)> {
    if physical.0 == 0 || physical.1 == 0 || scale <= 0.0 {
        return None;
    }
    Some((
        (f64::from(physical.0) / scale).round() as u32,
        (f64::from(physical.1) / scale).round() as u32,
    ))
}

#[derive(Props, Clone, PartialEq)]
pub struct StickyNoteProps {
    pub id: String,
    pub notes: Signal<NoteStore>,
    pub tasks: Signal<TaskStore>,
    /// Handle on the App-scoped pipeline. `Coroutine<T>` is `Copy` and its
    /// channel is not tied to a scope, so it crosses the VirtualDom boundary
    /// for the same reason a `Signal` does — and a pass asked for here still
    /// completes if this window is closed while it runs.
    pub passes: Coroutine<PipelineRequest>,
}

#[component]
pub fn StickyNote(props: StickyNoteProps) -> Element {
    let StickyNoteProps { id, mut notes, mut tasks, passes } = props;

    // Wayland gives a client no way to set its own position, but it may ask the
    // compositor to take over an interactive move — `drag()` wraps tao's
    // `drag_window()`, which is `xdg_toplevel.move` under Mutter. That is the
    // only way an undecorated note can be moved, since `with_decorations(false)`
    // means there is no compositor titlebar to grab.
    //
    // `use_window()` resolves to *this* sticky's own context, for the same
    // reason the close handler below does: each sticky is its own VirtualDom.
    let window = use_window();

    // Serve this note's images to this note's webview. Registered here rather
    // than in `App()` for the same per-window reason as the event handler
    // below — see `sticky_blocks`.
    sticky_blocks::use_note_media(id.clone(), notes);

    let note = {
        let id = id.clone();
        use_memo(move || notes.read().get(&id).cloned())
    };

    let mut drop_target = use_signal(|| false);
    // Set when a paste carried an image and no text. See `clipboard_paste`:
    // there is nothing on disk for a screenshot to reference, so the note says
    // so rather than swallowing the paste in silence.
    let mut paste_hint = use_signal(|| false);

    // Record that the user closed this window, and how big they made it.
    // Registered here, above the early return below — a hook after a
    // conditional return breaks the fixed-hook-order rule the moment the note
    // is archived.
    //
    // **Registered inside `StickyNote`, deliberately not in `App()`.**
    // `create_wry_event_handler` keys the handler to the window that registers
    // it, and `apply_event` skips any `WindowEvent` whose `window_id` differs.
    // A handler registered in `App()` would therefore only ever see the *main*
    // window's events — a silent no-op that looks entirely correct. Here,
    // `window()` resolves to this sticky's own context, so it sees its own
    // events and nothing else, and no `WindowId` map is needed.
    //
    // The close arm fires only for user and compositor closes. A programmatic
    // `ctx.close()` sends `UserEvent(CloseWindow)` straight to
    // `handle_close_requested` and never produces a `WindowEvent`, so the
    // archive path cannot double-fire through here and needs no guard flag.
    {
        let id = id.clone();
        let window = window.clone();
        use_wry_event_handler(move |event, _| match event {
            TaoEvent::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                notes.write().set_open(&id, false);
            }
            TaoEvent::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                let Some(logical) = logical_size((size.width, size.height), window.scale_factor())
                else {
                    return;
                };
                // ⚠️ Guarded with `peek`, before any `write()`. `Resized` fires
                // once per frame of a grip drag *and* again when the window
                // maps, and `Signal::write` notifies every subscriber whether
                // or not the value changed — so an unguarded call would
                // re-render this note and the whole board for a size that is
                // already recorded. `set_size`'s own guard is not enough: by
                // then the write lock has already been taken.
                if notes.peek().size(&id) == Some(logical) {
                    return;
                }
                notes.write().set_size(&id, logical);
            }
            _ => {}
        });
    }

    let Some(note) = note() else {
        // The note was archived or deleted from another window while this one
        // was open.
        return rsx! { div { class: "sticky-gone", "This note was deleted." } };
    };

    let color_class = if drop_target() {
        format!("sticky sticky-{} sticky-drop-target", note.color.css_class())
    } else {
        format!("sticky sticky-{}", note.color.css_class())
    };
    let archive_id = id.clone();
    let chips_id = id.clone();
    let pass_id = id.clone();
    let drop_id = id.clone();
    let pick_id = id.clone();
    let paste_id = id.clone();

    // Keyed to the stage fields, not to how the note was created. A dictated
    // note whose cleanup was superseded by an edit has spent its automatic
    // trigger; reading the stages is what leaves it a way back.
    let footer = sticky_footer::footer(note.clean_state, note.extract_state);

    rsx! {
        div {
            class: "{color_class}",
            // Without a `prevent_default` on dragover the browser refuses the
            // drop outright and `ondrop` never fires at all.
            ondragover: move |e| e.prevent_default(),
            ondragenter: move |_| drop_target.set(true),
            ondragleave: move |_| drop_target.set(false),
            ondrop: move |e: Event<DragData>| {
                e.prevent_default();
                drop_target.set(false);
                let dropped = attachments_from_drop(&e);
                if dropped.is_empty() {
                    return;
                }
                {
                    let mut store = notes.write();
                    for attachment in dropped {
                        store.add_attachment(&drop_id, attachment);
                    }
                }
                // Inline, like `do_note_capture`, and through `flush_stores`
                // for the same reason: `flush_if_dirty` writes only the JSON
                // mirror, which nothing reads back, so a photo you just
                // dropped would still be lost to a crash before the tick.
                crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
            },
            onpaste: move |e: Event<ClipboardData>| {
                // ⚠️ `ClipboardData` carries **nothing** on desktop —
                // `SerializedClipboardData` is an empty struct — so the
                // clipboard is read directly, the same way `home.rs` and
                // `history_page.rs` already do. Synchronously, because
                // `prevent_default` rides the event's own IPC response and a
                // spawned read would answer too late to suppress the insert.
                match clipboard_paste() {
                    Pasted::Url(url) => {
                        e.prevent_default();
                        paste_hint.set(false);
                        notes.write().add_attachment(
                            &paste_id,
                            Attachment::Link { id: next_id(), url, title: None },
                        );
                        crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
                    }
                    Pasted::ImageBytes => {
                        e.prevent_default();
                        paste_hint.set(true);
                    }
                    // Ordinary text. Falls through and inserts as it always has.
                    Pasted::Nothing => paste_hint.set(false),
                }
            },
            div {
                class: "sticky-bar",
                // The bar is the title bar: press and the compositor moves the
                // window. Positions are deliberately not persisted — see
                // agent_docs/sticky_notes.md — so a drag lasts the session.
                onmousedown: {
                    let window = window.clone();
                    move |_| window.drag()
                },
                div { class: "sticky-dots",
                    for c in PALETTE {
                        button {
                            key: "{c.css_class()}",
                            class: "sticky-dot sticky-dot-{c.css_class()}",
                            title: "{c.css_class()}",
                            // Without this the bar's mousedown starts a window
                            // drag and the compositor swallows the click, so
                            // the swatch would never fire.
                            onmousedown: move |e| e.stop_propagation(),
                            onclick: {
                                let id = id.clone();
                                move |_| { notes.write().set_color(&id, c); }
                            },
                        }
                    }
                }
                div { class: "sticky-bar-actions",
                    // The fallback for every drop, and the only path on
                    // Windows. dioxus-desktop turns this into a native dialog
                    // returning real paths — no new dependency.
                    label {
                        class: "sticky-attach",
                        title: "Attach a file",
                        onmousedown: move |e: Event<MouseData>| e.stop_propagation(),
                        "\u{1F4CE}"
                        input {
                            r#type: "file",
                            multiple: true,
                            class: "sticky-file-input",
                            onchange: move |e: Event<FormData>| {
                                let files = e.files();
                                if files.is_empty() {
                                    return;
                                }
                                {
                                    let mut store = notes.write();
                                    for file in files {
                                        store.add_attachment(
                                            &pick_id,
                                            sticky_blocks::attachment_for_path(
                                                next_id(),
                                                file.path(),
                                            ),
                                        );
                                    }
                                }
                                crate::notes::flush_stores(
                                    &mut notes.write(),
                                    &mut tasks.write(),
                                );
                            },
                        }
                    }
                    button {
                        class: "sticky-archive",
                        title: "Archive this note",
                        onmousedown: move |e| e.stop_propagation(),
                        onclick: move |_| { notes.write().archive(&archive_id); },
                        "\u{00d7}"
                    }
                }
            }
            StickyBody {
                id: id.clone(),
                notes,
                body: note.body.clone(),
                attachments: note.attachments.clone(),
            }
            StickyChips { note_id: chips_id, tasks }
            div { class: "sticky-footer",
                button {
                    class: if footer.icon == FooterIcon::Check {
                        "sticky-pass sticky-pass-done"
                    } else {
                        "sticky-pass"
                    },
                    title: "{footer.tooltip}",
                    onclick: move |_| {
                        passes.send(PipelineRequest::retry(pass_id.clone(), footer.stages));
                    },
                    if footer.icon == FooterIcon::Check {
                        IconCheck { size: 13 }
                    } else {
                        IconAsterisk { size: 13 }
                    }
                }
                if let Some(message) = footer.error {
                    span { class: "sticky-pass-error", "{message}" }
                } else if paste_hint() {
                    // Said out loud rather than left to be discovered. An
                    // attachment is a reference to a file, and a screenshot on
                    // the clipboard is not a file anywhere yet.
                    span { class: "sticky-paste-hint",
                        "Save the image first, then attach it with \u{1F4CE}"
                    }
                }
                // The grip. `with_decorations(false)` means the compositor
                // offers no edge to grab, so the window asks for the resize
                // itself — `xdg_toplevel.resize`, which unlike positioning is
                // client-initiated and needs no extension method. Exactly how
                // `.sticky-bar` already calls `drag()`.
                div {
                    class: "sticky-grip",
                    title: "Resize",
                    onmousedown: {
                        let window = window.clone();
                        move |e: Event<MouseData>| {
                            e.stop_propagation();
                            let _ = window.drag_resize_window(ResizeDirection::SouthEast);
                        }
                    },
                }
            }
        }
    }
}

/// What a drop is carrying, in the order the plan settled on: real files first,
/// then a URL from the drag's data transfer.
///
/// On every platform but Windows, wry's native drag-drop handler merges real
/// filesystem paths into the HTML event, so `files()` is authoritative when it
/// is non-empty. A drag from a browser carries no files and arrives as
/// `text/uri-list` or a bare `text/plain` URL instead.
fn attachments_from_drop(e: &Event<DragData>) -> Vec<Attachment> {
    let files = e.files();
    if !files.is_empty() {
        return files
            .into_iter()
            .map(|f| sticky_blocks::attachment_for_path(next_id(), f.path()))
            .collect();
    }

    let transfer = e.data_transfer();
    let text = transfer
        .get_data("text/uri-list")
        .or_else(|| transfer.get_data("text/plain"))
        .unwrap_or_default();

    sticky_blocks::url_in(&text)
        .map(|url| vec![Attachment::Link { id: next_id(), url, title: None }])
        .unwrap_or_default()
}

/// What a paste turned out to be carrying.
pub enum Pasted {
    /// A bare URL, and nothing else. Becomes a link chip.
    Url(String),
    /// Image bytes with no text beside them — a screenshot, or a copy out of an
    /// image editor.
    ImageBytes,
    /// Anything else, including ordinary prose that merely mentions a link.
    /// The paste falls through and inserts text as it always has.
    Nothing,
}

/// Read the system clipboard and decide what the paste means.
///
/// **Pasted image bytes are out of scope**, and deliberately: an attachment is a
/// reference to a file on disk, and a screenshot on the clipboard is not a file
/// anywhere. Detected rather than ignored, so the note can say so — silently
/// dropping a paste is the one outcome that would look like a bug.
fn clipboard_paste() -> Pasted {
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return Pasted::Nothing;
    };
    if let Ok(text) = clipboard.get_text() {
        if let Some(url) = sticky_blocks::url_in(&text) {
            return Pasted::Url(url);
        }
        if !text.trim().is_empty() {
            return Pasted::Nothing;
        }
    }
    if clipboard.get_image().is_ok() {
        return Pasted::ImageBytes;
    }
    Pasted::Nothing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_title_embeds_the_note_id() {
        let title = window_title("18f2a1b3c4d-0001");
        assert_eq!(title, "Beamer Note 18f2a1b3c4d-0001");
        assert!(
            title.starts_with(TITLE_PREFIX),
            "the GNOME extension matches on this prefix; changing it breaks placement"
        );
    }

    #[test]
    fn titles_are_unique_per_note() {
        assert_ne!(window_title("a"), window_title("b"));
    }

    #[test]
    fn a_resize_at_scale_two_stores_half_the_physical_numbers() {
        // Invisible on this machine, where everything is scale 1.0 — and wrong
        // on every HiDPI display, reopening each note at double size.
        assert_eq!(logical_size((800, 640), 2.0), Some((400, 320)));
        assert_eq!(logical_size((400, 320), 1.0), Some((400, 320)));
        assert_eq!(logical_size((600, 480), 1.5), Some((400, 320)));
    }

    #[test]
    fn a_zero_sized_resize_is_ignored() {
        // A minimize on some compositors. Storing it would reopen the note as
        // a sliver with no way back to a usable size.
        assert_eq!(logical_size((0, 640), 2.0), None);
        assert_eq!(logical_size((800, 0), 2.0), None);
        assert_eq!(logical_size((800, 640), 0.0), None, "a bad scale must not divide by zero");
    }
}

//! Sticky note windows.
//!
//! One ordinary Dioxus window per open note — the same `new_window` pattern
//! used for the splash and pill windows in `app_setup.rs`. Deliberately NOT
//! always-on-top: notes sit in the normal stacking order.
//!
//! Wayland gives clients no control over their own position, so placement goes
//! through Beamer's GNOME extension (`shell_window`); the window title is the
//! handle it matches on. There is no geometry read-back — notes are placed by
//! `note_layout`, never restored to where they were.
//!
//! The store arrives as a **prop**, not via `use_context`. Each sticky window is
//! its own `VirtualDom` with its own scope tree, and `use_context` walks only
//! the current dom's tree — the main window's provider is invisible from here.
//! A `Signal` is `Copy + 'static` and its generational-box arena is thread-local
//! (`generational-box`'s `UNSYNC_RUNTIME`), and every desktop `VirtualDom` polls
//! on the main thread, so the handle resolves across the dom boundary even
//! though the context does not.

use dioxus::desktop::tao::event::{Event, WindowEvent};
use dioxus::desktop::{use_window, use_wry_event_handler};
use dioxus::prelude::*;

use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::{NoteColor, NoteStore};
use crate::ui::icons::{IconAsterisk, IconCheck};
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
    let StickyNoteProps { id, mut notes, tasks, passes } = props;

    // Wayland gives a client no way to set its own position, but it may ask the
    // compositor to take over an interactive move — `drag()` wraps tao's
    // `drag_window()`, which is `xdg_toplevel.move` under Mutter. That is the
    // only way an undecorated note can be moved, since `with_decorations(false)`
    // means there is no compositor titlebar to grab.
    //
    // `use_window()` resolves to *this* sticky's own context, for the same
    // reason the close handler below does: each sticky is its own VirtualDom.
    let window = use_window();

    let note = {
        let id = id.clone();
        use_memo(move || notes.read().get(&id).cloned())
    };

    // Record that the user closed this window, so the board can offer to bring
    // it back. Registered here, above the early return below — a hook after a
    // conditional return breaks the fixed-hook-order rule the moment the note
    // is archived.
    //
    // **Registered inside `StickyNote`, deliberately not in `App()`.**
    // `create_wry_event_handler` keys the handler to the window that registers
    // it, and `apply_event` skips any `WindowEvent` whose `window_id` differs.
    // A handler registered in `App()` would therefore only ever see the *main*
    // window's events — a silent no-op that looks entirely correct. Here,
    // `window()` resolves to this sticky's own context, so it sees its own
    // close and nothing else, and no `WindowId` map is needed.
    //
    // This arm fires only for user and compositor closes. A programmatic
    // `ctx.close()` sends `UserEvent(CloseWindow)` straight to
    // `handle_close_requested` and never produces a `WindowEvent`, so the
    // archive path cannot double-fire through here and needs no guard flag.
    {
        let id = id.clone();
        use_wry_event_handler(move |event, _| {
            if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
                notes.write().set_open(&id, false);
            }
        });
    }

    let Some(note) = note() else {
        // The note was archived from another window while this one was open.
        return rsx! { div { class: "sticky-gone", "This note was deleted." } };
    };

    let body = note.body.clone();
    let color_class = format!("sticky sticky-{}", note.color.css_class());
    let edit_id = id.clone();
    let archive_id = id.clone();
    let chips_id = id.clone();
    let pass_id = id.clone();

    // Keyed to the stage fields, not to how the note was created. A dictated
    // note whose cleanup was superseded by an edit has spent its automatic
    // trigger; reading the stages is what leaves it a way back.
    let footer = sticky_footer::footer(note.clean_state, note.extract_state);

    rsx! {
        div { class: "{color_class}",
            div {
                class: "sticky-bar",
                // The bar is the title bar: press and the compositor moves the
                // window. Positions are deliberately not persisted — see
                // agent_docs/sticky_notes.md — so a drag lasts the session.
                onmousedown: move |_| window.drag(),
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
                button {
                    class: "sticky-archive",
                    title: "Archive this note",
                    onmousedown: move |e| e.stop_propagation(),
                    onclick: move |_| { notes.write().archive(&archive_id); },
                    "\u{00d7}"
                }
            }
            textarea {
                class: "sticky-body",
                spellcheck: false,
                value: "{body}",
                oninput: move |e| { notes.write().set_body(&edit_id, e.value()); },
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
                        passes.send(PipelineRequest {
                            note_id: pass_id.clone(),
                            stages: footer.stages,
                        });
                    },
                    if footer.icon == FooterIcon::Check {
                        IconCheck { size: 13 }
                    } else {
                        IconAsterisk { size: 13 }
                    }
                }
                if let Some(message) = footer.error {
                    span { class: "sticky-pass-error", "{message}" }
                }
            }
        }
    }
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
}

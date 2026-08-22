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

use crate::notes::{NoteColor, NoteStore};

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
}

#[component]
pub fn StickyNote(props: StickyNoteProps) -> Element {
    let StickyNoteProps { id, mut notes } = props;

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
        }
    }
}

/// Sticky note styling, in Deploy Purple.
///
/// Injected as an inline `<style>` per window, so it must be self-contained —
/// it cannot `@import` the app stylesheet. `embedded_font_css()` is prepended
/// at window creation to supply the faces this references.
///
/// Corner radius is 8px because `agent_docs/design_system.md` caps it there
/// ("Sharp system: 4px base, 6px cards, 8px max. Never rounder."). Notes take
/// the maximum, which makes them the softest surface in the app without
/// leaving its language.
pub const STICKY_CSS: &str = r#"
:root {
  /* Mirrors the app tokens in assets/styles.css. Duplicated, not imported:
     an inline <style> has no access to the main stylesheet. */
  --ink:#0f152a;
  --ink-soft:#64708b;
  --accent:#4B0082;
  --danger:#DC2626;
  --note-border:rgba(75,0,130,0.28);
  --note-shadow:rgba(75,0,130,0.18);
  --note-chrome:rgba(255,255,255,0.42);
  --radius-lg:8px;
  --duration-fast:150ms;
  --ease:cubic-bezier(0.25,1,0.5,1);
}

*, *::before, *::after { margin:0; padding:0; box-sizing:border-box; }

/* Transparent all the way down. The window is built with_transparent(true)
   and a (0,0,0,0) background color; without these the webview still paints an
   opaque white sheet and the rounded corners show it as square white nubs. */
html, body, #main { height:100%; overflow:hidden; background:transparent; }

body { font-family:"Recursive","Segoe UI Variable","Segoe UI",system-ui,sans-serif;
  -webkit-font-smoothing:antialiased; }

/* Room for the hard-offset shadow to render inside the window. Without this
   the shadow is clipped by the window edge and simply never appears. */
#main { padding:0 5px 5px 0; }

.sticky { display:flex; flex-direction:column; height:100%;
  border:2px solid var(--note-border);
  border-radius:var(--radius-lg);
  /* Clips the bar's fill and border-bottom to the rounded top corners —
     without it the bar squares them off again. */
  overflow:hidden;
  box-shadow:3px 4px 0 0 var(--note-shadow); }

.sticky-purple { background:#EDE4FB; } .sticky-violet { background:#E4E6FB; }
.sticky-amber  { background:#FBF1DC; } .sticky-teal   { background:#DCF5F0; }
.sticky-rose   { background:#FBE1E8; } .sticky-slate  { background:#E7E9EC; }

/* The title bar. `cursor:grab` and `user-select:none` are the affordance for
   the window drag wired up in StickyNote — a bar that selects text on
   press-and-move reads as broken even when the drag works. */
.sticky-bar { display:flex; align-items:center; justify-content:space-between;
  padding:7px 9px; border-bottom:2px solid var(--note-border);
  background:var(--note-chrome);
  cursor:grab; user-select:none; -webkit-user-select:none; }
.sticky-bar:active { cursor:grabbing; }

.sticky-dots { display:flex; gap:6px; }
.sticky-dot { width:12px; height:12px; border:1.5px solid var(--note-border);
  border-radius:50%; cursor:pointer; padding:0;
  transition:transform var(--duration-fast) var(--ease); }
.sticky-dot:hover { transform:scale(1.18); }
.sticky-dot-purple{background:#8921E4} .sticky-dot-violet{background:#6B6BE4}
.sticky-dot-amber {background:#E4A421} .sticky-dot-teal  {background:#21C9B0}
.sticky-dot-rose  {background:#E4216B} .sticky-dot-slate {background:#8A93A0}

/* Close hover goes danger red, per the custom-title-bar spec. */
.sticky-archive { background:none; border:none; cursor:pointer;
  font-family:"DM Mono","Cascadia Code",monospace;
  font-size:16px; line-height:1; color:var(--ink-soft); padding:2px 5px;
  border-radius:var(--radius-lg);
  transition:color var(--duration-fast) var(--ease),
             background var(--duration-fast) var(--ease); }
.sticky-archive:hover { color:var(--danger); background:rgba(220,38,38,0.10); }

.sticky-body { flex:1; width:100%; resize:none; border:none; outline:none;
  background:transparent; padding:12px; font-family:inherit; font-size:14px;
  line-height:1.5; color:var(--ink); caret-color:var(--accent); }
.sticky-body::selection { background:rgba(75,0,130,0.18); }
.sticky-body::placeholder { color:#94a0b8; }

.sticky-gone { display:flex; align-items:center; justify-content:center;
  height:100%; padding:16px; text-align:center;
  font-size:13px; color:var(--ink-soft);
  background:#E7E9EC; border:2px solid var(--note-border);
  border-radius:var(--radius-lg); }
"#;

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

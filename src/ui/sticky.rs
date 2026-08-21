//! Sticky note windows.
//!
//! One ordinary Dioxus window per open note — the same `new_window` pattern
//! used for the splash and pill windows in `app_setup.rs`. Deliberately NOT
//! always-on-top: notes sit in the normal stacking order.
//!
//! Wayland gives clients no control over their own position, so placement and
//! geometry read-back both go through Beamer's GNOME extension (`shell_window`).
//! The window title is the handle the extension matches on.
//!
//! The store arrives as a **prop**, not via `use_context`. Each sticky window is
//! its own `VirtualDom` with its own scope tree, and `use_context` walks only
//! the current dom's tree — the main window's provider is invisible from here.
//! A `Signal` is `Copy + 'static` and its generational-box arena is thread-local
//! (`generational-box`'s `UNSYNC_RUNTIME`), and every desktop `VirtualDom` polls
//! on the main thread, so the handle resolves across the dom boundary even
//! though the context does not.

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

    let note = {
        let id = id.clone();
        use_memo(move || notes.read().get(&id).cloned())
    };

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
            div { class: "sticky-bar",
                div { class: "sticky-dots",
                    for c in PALETTE {
                        button {
                            key: "{c.css_class()}",
                            class: "sticky-dot sticky-dot-{c.css_class()}",
                            title: "{c.css_class()}",
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

pub const STICKY_CSS: &str = r#"
*, *::before, *::after { margin:0; padding:0; box-sizing:border-box; }
html, body, #main { height:100%; overflow:hidden;
  font-family:"DM Mono","Segoe UI Variable","Segoe UI",monospace,system-ui,sans-serif; }

.sticky { display:flex; flex-direction:column; height:100%;
  border:2px solid #1a1a1f; box-shadow:4px 4px 0 #1a1a1f; }
.sticky-purple { background:#EDE4FB; } .sticky-violet { background:#E4E6FB; }
.sticky-amber  { background:#FBF1DC; } .sticky-teal   { background:#DCF5F0; }
.sticky-rose   { background:#FBE1E8; } .sticky-slate  { background:#E7E9EC; }

.sticky-bar { display:flex; align-items:center; justify-content:space-between;
  padding:6px 8px; border-bottom:2px solid #1a1a1f; }
.sticky-dots { display:flex; gap:5px; }
.sticky-dot { width:12px; height:12px; border:1.5px solid #1a1a1f; border-radius:50%;
  cursor:pointer; padding:0; }
.sticky-dot-purple{background:#8921E4} .sticky-dot-violet{background:#6B6BE4}
.sticky-dot-amber {background:#E4A421} .sticky-dot-teal  {background:#21C9B0}
.sticky-dot-rose  {background:#E4216B} .sticky-dot-slate {background:#8A93A0}

.sticky-archive { background:none; border:none; cursor:pointer;
  font-size:18px; line-height:1; color:#1a1a1f; padding:0 4px; }

.sticky-body { flex:1; width:100%; resize:none; border:none; outline:none;
  background:transparent; padding:10px; font:inherit; font-size:14px;
  line-height:1.5; color:#1a1a1f; }

.sticky-gone { padding:16px; font-size:13px; color:#6b6b73; }
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

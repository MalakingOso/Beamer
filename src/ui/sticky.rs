//! Sticky note windows: one ordinary (not always-on-top) Dioxus window per open
//! note. Wayland clients can't position themselves, so placement goes through
//! the GNOME extension (`shell_window`), matched by window title; position is
//! never read back, but size is (`WindowEvent::Resized`) and is remembered.
//! Each window is its own `VirtualDom`, so the store arrives as a prop —
//! `use_context` can't see the main window's providers across that boundary.

use std::collections::HashSet;
use std::time::Duration;

use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
use dioxus::desktop::tao::window::ResizeDirection;
use dioxus::desktop::{use_window, use_wry_event_handler};
use dioxus::html::HasFileData;
use dioxus::prelude::*;

use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::{next_id, Attachment, NoteColor, NoteOrigin, NoteStore, StageState};
use crate::ui::icons::{IconAsterisk, IconCheck, IconPlus};
use crate::ui::sticky_blocks::{self, StickyBody};
use crate::ui::sticky_chips::StickyChips;
use crate::ui::sticky_footer::{self, FooterIcon};

pub const TITLE_PREFIX: &str = "Beamer Note ";

pub fn window_title(id: &str) -> String {
    format!("{TITLE_PREFIX}{id}")
}

pub const PALETTE: [NoteColor; 6] = NoteColor::ALL;

/// Convert a `Resized` event's physical size to the logical one the store keeps.
/// ⚠️ `Resized` is physical, the window was built logical — swapped, notes
/// reopen at double/half size on HiDPI (invisible at 1x). A zero axis is a
/// minimize, not a resize, and must not be stored.
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
    /// App-scoped pipeline handle. `Coroutine` is `Copy` and crosses the
    /// VirtualDom boundary like a `Signal`; a requested pass outlives this window.
    pub passes: Coroutine<PipelineRequest>,
    /// Note ids the pipeline coroutine currently has in flight. A reactive
    /// mirror of its own non-`Signal` dedup set, threaded down purely so this
    /// window can show a running indicator.
    pub passes_in_flight: Signal<HashSet<String>>,
}

/// How long the footer keeps showing red failure text before going quiet.
/// Middle of the 5-10s range the user asked for — the sweep is still the
/// real recovery mechanism, this only calms the visible alarm.
const FAILURE_TEXT_TIMEOUT: Duration = Duration::from_secs(7);

#[component]
pub fn StickyNote(props: StickyNoteProps) -> Element {
    let StickyNoteProps { id, mut notes, mut tasks, passes, passes_in_flight } = props;

    // `drag()` (compositor interactive move) is the only way to move an
    // undecorated note. `use_window()` resolves to this sticky's own VirtualDom.
    let window = use_window();

    // Per-window asset handler — must register here, not in `App()` (see `sticky_blocks`).
    sticky_blocks::use_note_media(id.clone(), notes);

    let note = {
        let id = id.clone();
        use_memo(move || notes.read().get(&id).cloned())
    };

    let mut drop_target = use_signal(|| false);
    // Set when a paste carried image bytes and no text (nothing on disk to attach).
    let mut paste_hint = use_signal(|| false);

    // Whether the footer's red failure text is currently shown. Starts true
    // so a note opened already-Failed still shows it once; the effect below
    // hides it ~7s after each fresh entry into Failed and shows it again on
    // the next one (e.g. a retry that fails again).
    let mut show_error_text = use_signal(|| true);
    {
        let id = id.clone();
        // Detected via the in-flight signal, not the note's own fields: a
        // retry that fails the same way it failed before leaves `clean_state`
        // unchanged (`Failed` -> `Failed`), so the note itself carries no
        // detectable transition. Passing through `in_flight` on every retry
        // does.
        let mut was_running = use_signal(|| false);
        let mut generation = use_signal(|| 0u64);
        use_effect(move || {
            let running = passes_in_flight.read().contains(&id);
            let just_finished = was_running.peek().to_owned() && !running;
            was_running.set(running);
            if !just_finished {
                return;
            }
            let failed = notes.peek().get(&id).is_some_and(|n| {
                n.clean_state == StageState::Failed || n.extract_state == StageState::Failed
            });
            if !failed {
                return;
            }
            show_error_text.set(true);
            let this_generation = *generation.peek() + 1;
            generation.set(this_generation);
            spawn(async move {
                tokio::time::sleep(FAILURE_TEXT_TIMEOUT).await;
                // A newer failure may have already bumped the generation and
                // restarted its own timer; only the latest one may hide the text.
                if *generation.peek() == this_generation {
                    show_error_text.set(false);
                }
            });
        });
    }

    // Hooks stay above the early return below (fixed hook order). The event
    // handler must register here, not in `App()`: handlers are keyed to the
    // registering window, so `App()` would only ever see the main window's events.
    // No double-fire guard needed — programmatic `close()` never produces a
    // `WindowEvent`, only user/compositor closes do.
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
                // `Resized` fires per frame of a drag; `peek` first so an
                // unchanged size doesn't re-render the note and the board.
                if notes.peek().size(&id) == Some(logical) {
                    return;
                }
                notes.write().set_size(&id, logical);
            }
            _ => {}
        });
    }

    // Windows/macOS only: dioxus hides every non-first window when its webview
    // loads (it inherits the hidden main window's `is_visible_before_start`), so
    // re-show here in an effect, which runs after that handler. Visible only —
    // no `set_focus()`: tao's Windows focus fakes an Alt keypress, which risks
    // interfering with an injection in flight.
    #[cfg(not(target_os = "linux"))]
    {
        let window = window.clone();
        use_effect(move || window.set_visible(true));
    }

    let Some(note) = note() else {
        // Archived or deleted from another window while this one was open.
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

    // Keyed to the stage fields, not the note's origin, so a superseded pass stays retryable.
    let in_flight = passes_in_flight.read().contains(&id);
    let footer = sticky_footer::footer(note.clean_state, note.extract_state, in_flight);

    rsx! {
        div {
            class: "{color_class}",
            // Without this, the browser refuses the drop and `ondrop` never fires.
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
                // Flush both stores so a crash before the tick can't lose the drop.
                crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
            },
            onpaste: move |e: Event<ClipboardData>| {
                // `ClipboardData` is empty on desktop, so read the clipboard
                // directly — synchronously, or `prevent_default` lands too late.
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
                    Pasted::Nothing => paste_hint.set(false),
                }
            },
            div {
                class: "sticky-bar",
                // The bar is the title bar; positions are not persisted.
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
                            // Without this the bar's drag swallows the click.
                            onmousedown: move |e| e.stop_propagation(),
                            onclick: {
                                let id = id.clone();
                                move |_| { notes.write().set_color(&id, c); }
                            },
                        }
                    }
                }
                div { class: "sticky-bar-actions",
                    button {
                        class: "sticky-new",
                        title: "New note",
                        onmousedown: move |e| e.stop_propagation(),
                        onclick: move |_| {
                            notes.write().create(
                                String::new(),
                                NoteColor::random(),
                                NoteOrigin::Typed,
                            );
                            crate::notes::flush_stores(
                                &mut notes.write(),
                                &mut tasks.write(),
                            );
                        },
                        IconPlus { size: 13 }
                    }
                    // Native file dialog (the only attach path on Windows).
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
                    class: match footer.icon {
                        FooterIcon::Check => "sticky-pass sticky-pass-done",
                        FooterIcon::Running => "sticky-pass sticky-pass-running",
                        FooterIcon::Asterisk => "sticky-pass",
                    },
                    title: "{footer.tooltip}",
                    onclick: move |_| {
                        passes.send(PipelineRequest::retry(pass_id.clone(), footer.stages));
                    },
                    if footer.icon == FooterIcon::Check {
                        IconCheck { size: 13 }
                    } else {
                        // `Running` reuses the asterisk glyph; the pulse/color come from CSS.
                        IconAsterisk { size: 13 }
                    }
                }
                if let Some(message) = footer.error {
                    if show_error_text() {
                        span { class: "sticky-pass-error", "{message}" }
                    }
                } else if paste_hint() {
                    span { class: "sticky-paste-hint",
                        "Save the image first, then attach it with \u{1F4CE}"
                    }
                }
                // No decorations means no edge to grab; resize is client-initiated
                // (unlike positioning) so no extension method is needed.
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

/// What a drop carries: real files first, else a URL from the data transfer.
/// Off Windows, `files()` is authoritative when non-empty; a browser drag
/// arrives as `text/uri-list` or bare `text/plain` instead.
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
    /// Image bytes with no text. Detected so the note can say so — a silent
    /// drop would look like a bug. (Attachments reference files on disk.)
    ImageBytes,
    /// Anything else; the paste falls through and inserts text as normal.
    Nothing,
}

/// Read the system clipboard and decide what the paste means.
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
#[path = "sticky_tests.rs"]
mod tests;

//! Reconcile sticky note windows against the note store: one effect makes the
//! live windows match the notes that should be showing (covers both new and
//! restored notes). Invariant: the effect reads `notes`, peeks the registry,
//! writes only the registry — writing `notes` inside its own trigger loops.
//! Notes are placed (`note_layout::place_next`), never restored to where they were.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use dioxus::desktop::tao::dpi::{LogicalPosition, LogicalSize};
#[cfg(target_os = "windows")]
use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
use dioxus::desktop::{Config as DesktopConfig, DesktopContext, WeakDesktopContext, WindowBuilder};
use dioxus::prelude::*;

use crate::config::Config;
use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::{Note, NoteStore};
use crate::ui::note_layout;
use crate::ui::work_area::{launch_seed, main_window_points, work_area};
use crate::ui::sticky::{window_title, StickyNote, StickyNoteProps};
use crate::ui::sticky_css::STICKY_CSS;

/// Size a note gets when it has none of its own.
pub const DEFAULT_NOTE_SIZE: (u32, u32) = (320, 260);

/// Smallest a note may be dragged to (bar, one line, footer, reachable grip).
pub const MIN_NOTE_SIZE: (u32, u32) = (180, 140);

/// One note's entry in the window registry.
#[derive(Clone)]
pub struct StickySlot {
    /// Where this note's window was told to go (logical coords). In memory, not
    /// in `notes.json` — the scatter is recomputed every launch; this is the
    /// only record `place_next` has of where open notes are.
    pub pos: (i32, i32),
    /// `None` while the window is still being constructed. Reserved before
    /// `new_window`'s await so a second pass landing mid-open doesn't double-open.
    /// Weak so a closed window's OS handle can actually drop.
    pub ctx: Option<WeakDesktopContext>,
}

/// Maps note id -> its window.
pub type StickyRegistry = Signal<HashMap<String, StickySlot>>;

/// Open one note's window at an already-chosen position. Caller must have
/// already reserved `note.id` in the registry with `pos` (see `setup_sticky_windows`).
#[allow(clippy::too_many_arguments)]
async fn open_note_window(
    window: DesktopContext,
    mut registry: StickyRegistry,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    passes: Coroutine<PipelineRequest>,
    passes_in_flight: Signal<HashSet<String>>,
    note: Note,
    pos: (i32, i32),
    all_workspaces: bool,
) {
    let (w, h) = notes.peek().size(&note.id).unwrap_or(DEFAULT_NOTE_SIZE);
    let title = window_title(&note.id);
    #[allow(unused_mut)]
    let mut builder = WindowBuilder::new()
        .with_title(title.clone())
        .with_decorations(false)
        .with_always_on_top(false)
        // Transparent surface + transparent backdrop = real rounded corners
        // (either alone leaves square nubs). Same combination the pill uses.
        .with_transparent(true)
        .with_inner_size(LogicalSize::new(w as f64, h as f64))
        .with_min_inner_size(LogicalSize::new(MIN_NOTE_SIZE.0 as f64, MIN_NOTE_SIZE.1 as f64))
        // Honored on Windows; ignored by Mutter (the GNOME extension covers Linux).
        .with_position(LogicalPosition::new(f64::from(pos.0), f64::from(pos.1)));
    // Windows only: notes skip the taskbar — the notes board is the way back
    // to a buried note.
    #[cfg(target_os = "windows")]
    {
        builder = builder.with_skip_taskbar(true);
        // Native drop-shadow reserves an inset rect that reads as a stray border
        // around a CSS-drawn rounded note.
        builder = builder.with_undecorated_shadow(false);
    }

    let cfg = DesktopConfig::new()
        .with_data_directory(super::webview_data_dir())
        .with_window(builder)
        .with_background_color((0, 0, 0, 0))
        // Inline <style> carries no @font-face, so prepend the embedded faces.
        .with_custom_head(format!(
            "<style>{}{}</style>",
            crate::ui::fonts::embedded_font_css(),
            STICKY_CSS
        ))
        // Tray app: archiving the last sticky while main is hidden must not exit.
        .with_exits_when_last_window_closes(false);

    let dom = VirtualDom::new_with_props(
        StickyNote,
        StickyNoteProps { id: note.id.clone(), notes, tasks, passes, passes_in_flight },
    );

    let ctx: DesktopContext = window.new_window(dom, cfg).await;
    tracing::info!("Opened sticky window for note {} at {:?}", note.id, pos);
    registry.write().insert(
        note.id.clone(),
        StickySlot { pos, ctx: Some(Rc::downgrade(&ctx)) },
    );

    // Mutter ignores the requested position; the shell extension moves it instead
    // (`place` retries until the window is mapped, then verifies — see `shell_window`).
    #[cfg(target_os = "linux")]
    crate::ui::shell_window::place(title, pos.0, pos.1, all_workspaces);
    #[cfg(not(target_os = "linux"))]
    let _ = (title, all_workspaces);
}

/// Close a note's window and drop its registry slot. An un-upgradeable weak
/// handle just means the window is already gone — not an error either way.
pub fn close_note_window(mut registry: StickyRegistry, id: &str) {
    if let Some(StickySlot { ctx: Some(weak), .. }) = registry.write().remove(id) {
        match weak.upgrade() {
            Some(ctx) => {
                tracing::info!("Closing sticky window for note {}", id);
                ctx.close();
            }
            None => tracing::debug!("Sticky window for note {} was already gone", id),
        }
    }
}

/// Whether the registry currently holds a usable window for a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// No entry at all.
    Absent,
    /// Reserved, but the window is still being constructed.
    Opening,
    /// An entry whose window still exists.
    Live,
    /// An entry whose window died without a close event (force-kill, compositor crash).
    Dead,
}

/// What clicking a note's card on the board should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReopenAction {
    /// Raise the window that already exists.
    Focus,
    /// An open is already in flight; let it finish.
    Nothing,
    /// Drop a stale slot, then mark the note open so the reconciler builds a
    /// fresh window.
    PruneAndReopen,
    /// Mark the note open and let the reconciler do the rest.
    Reopen,
}

/// Decide what a board click means. Split out so it can be tested without a
/// live event loop or a real window.
pub fn reopen_action(slot: SlotState, open: bool) -> ReopenAction {
    match (slot, open) {
        (SlotState::Live, true) => ReopenAction::Focus,
        // Live slot on a closed note = window mid-teardown; don't focus what is vanishing.
        (SlotState::Live, false) => ReopenAction::PruneAndReopen,
        (SlotState::Opening, _) => ReopenAction::Nothing,
        (SlotState::Dead, _) => ReopenAction::PruneAndReopen,
        (SlotState::Absent, _) => ReopenAction::Reopen,
    }
}

/// Whether the reconciler may open windows: never over the splash. `ready` is
/// `app_ready`; waiting loses nothing since the effect re-runs when it flips.
pub fn may_open(ready: bool, pending: usize) -> bool {
    ready && pending > 0
}

fn slot_state(registry: &StickyRegistry, id: &str) -> SlotState {
    match registry.peek().get(id) {
        None => SlotState::Absent,
        Some(StickySlot { ctx: None, .. }) => SlotState::Opening,
        Some(StickySlot { ctx: Some(weak), .. }) => match weak.upgrade() {
            Some(_) => SlotState::Live,
            None => SlotState::Dead,
        },
    }
}

/// Bring a note back — the notes board's click handler. Dead slots are pruned
/// here, not in the reconciler (which must leave stale slots alone lest it
/// fight a user who just closed a window).
pub fn reopen_note(mut registry: StickyRegistry, mut notes: Signal<NoteStore>, id: &str) {
    let open = notes.peek().is_open(id);
    match reopen_action(slot_state(&registry, id), open) {
        ReopenAction::Focus => {
            if let Some(StickySlot { ctx: Some(weak), .. }) = registry.peek().get(id) {
                if let Some(ctx) = weak.upgrade() {
                    ctx.set_visible(true);
                    ctx.set_focus();
                }
            }
        }
        ReopenAction::Nothing => {}
        ReopenAction::PruneAndReopen => {
            registry.write().remove(id);
            notes.write().set_open(id, true);
        }
        ReopenAction::Reopen => {
            notes.write().set_open(id, true);
        }
    }
}

/// Watch the note store and keep open windows matching it. Call once from
/// `App()`; returns the registry for the notes board.
#[allow(clippy::too_many_arguments)]
pub fn setup_sticky_windows(
    window: DesktopContext,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    passes: Coroutine<PipelineRequest>,
    passes_in_flight: Signal<HashSet<String>>,
    config: Signal<Config>,
    app_ready: Signal<bool>,
) -> StickyRegistry {
    let registry: StickyRegistry = use_signal(HashMap::new);

    use_effect(move || {
        // Read unconditionally so the effect reruns the moment the splash closes.
        let ready = *app_ready.read();

        // The store is the trigger; the registry is only peeked so opening a
        // window doesn't re-run this effect.
        let store = notes.read();

        let mut to_open: Vec<Note> = Vec::new();
        let mut to_close: Vec<String> = Vec::new();
        {
            // Dead handles are NOT pruned here — that would instantly reopen a
            // window the user just closed. `StickyNote`'s close handler marks it
            // closed; force-killed windows are pruned by `reopen_note` instead.
            let live = registry.peek();
            for note in store.notes.iter() {
                let showing = store.is_open(&note.id) && !note.archived;
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

        // Nothing opens over the splash.
        if !may_open(ready, to_open.len()) {
            return;
        }

        let area = work_area(&window);
        // Notes keep clear of the main window so they can't open hidden under it.
        let main_window = main_window_points(&window);
        let launch = launch_seed();
        // `peek`: config is policy, not a trigger.
        let all_workspaces = config.peek().notes.all_workspaces;

        // Reserve every slot synchronously before any await — otherwise a
        // restart's single pass would place all notes against the same empty
        // occupied set and stack them in one spot.
        let mut pending: Vec<(Note, (i32, i32))> = Vec::new();
        for (index, note) in to_open.into_iter().enumerate() {
            let mut reg = registry;
            if reg.peek().contains_key(&note.id) {
                continue;
            }

            let mut occupied: Vec<(i32, i32)> = reg.peek().values().map(|s| s.pos).collect();
            occupied.extend_from_slice(&main_window);
            let size = notes.peek().size(&note.id).unwrap_or(DEFAULT_NOTE_SIZE);
            let pos = note_layout::place_next(
                area,
                size,
                &occupied,
                note_layout::seed_for(launch, index),
            );
            reg.write().insert(note.id.clone(), StickySlot { pos, ctx: None });
            pending.push((note, pos));
        }

        if pending.is_empty() {
            return;
        }

        // Opened serially in one task, not one spawn per note — concurrent
        // opens hit dioxus#2483 on Windows (`WebView2Error(0x8007139F)`).
        let window = window.clone();
        spawn(async move {
            for (note, pos) in pending {
                open_note_window(
                    window.clone(),
                    registry,
                    notes,
                    tasks,
                    passes,
                    passes_in_flight,
                    note,
                    pos,
                    all_workspaces,
                )
                .await;
            }
        });
    });

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_window_is_raised_rather_than_reopened() {
        assert_eq!(reopen_action(SlotState::Live, true), ReopenAction::Focus);
    }

    #[test]
    fn a_window_that_died_without_a_close_event_is_pruned_first() {
        // Without the prune the reconciler never opens a replacement.
        assert_eq!(
            reopen_action(SlotState::Dead, false),
            ReopenAction::PruneAndReopen
        );
        assert_eq!(
            reopen_action(SlotState::Dead, true),
            ReopenAction::PruneAndReopen
        );
    }

    #[test]
    fn a_window_mid_teardown_is_replaced_not_focused() {
        assert_eq!(
            reopen_action(SlotState::Live, false),
            ReopenAction::PruneAndReopen,
            "its close handler has run; focusing it would raise a window that is about to vanish"
        );
    }

    #[test]
    fn an_unregistered_note_just_gets_marked_open() {
        assert_eq!(reopen_action(SlotState::Absent, false), ReopenAction::Reopen);
    }

    #[test]
    fn nothing_opens_while_the_splash_is_still_up() {
        assert!(!may_open(false, 1));
        assert!(!may_open(false, 7));
    }

    #[test]
    fn everything_waiting_opens_once_the_splash_closes() {
        assert!(may_open(true, 1));
        assert!(may_open(true, 7));
    }

    #[test]
    fn a_pass_with_nothing_to_open_does_no_work() {
        assert!(!may_open(true, 0));
        assert!(!may_open(false, 0));
    }

    #[test]
    fn an_open_already_in_flight_is_left_alone() {
        assert_eq!(reopen_action(SlotState::Opening, false), ReopenAction::Nothing);
        assert_eq!(reopen_action(SlotState::Opening, true), ReopenAction::Nothing);
    }
}

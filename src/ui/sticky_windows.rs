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
//!
//! **Reconciler invariant.** The effect **reads** `notes`, **peeks** the
//! registry, and **writes only the registry**. Every write to `notes` happens
//! outside it. A write to `notes` from inside its own trigger is an
//! unconditional re-trigger loop, and a `read()` of the registry would make
//! opening a window re-run the effect that opened it.
//!
//! **Notes are placed, not remembered.** Nothing restores a note to where it
//! was; `note_layout::place_next` scatters each new window around the ones
//! already on screen. See that module for why choosing the layout beats trying
//! to recover it, and `shell_window` for why the compositor has to do the
//! moving.

use std::collections::HashMap;
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

/// Smallest a note may be dragged to. Enough for the bar, one line and the
/// footer — below that the grip itself stops being reachable.
pub const MIN_NOTE_SIZE: (u32, u32) = (180, 140);

/// One note's entry in the window registry.
#[derive(Clone)]
pub struct StickySlot {
    /// Where this note's window was told to go, in logical coordinates.
    ///
    /// Deliberately in memory and not in `notes.json` — the scatter is
    /// recomputed from scratch every launch. What it is *for* is the next
    /// placement: `place_next` has to know where the open notes are, and this
    /// is the only record of that. Beamer cannot ask the compositor (see
    /// `shell_window`), and would not want to even if it could: the user may
    /// have dragged the note since, and space they dragged it out of is
    /// exactly the space the next note should be free to use.
    pub pos: (i32, i32),
    /// `None` while the window is still being constructed.
    ///
    /// The slot is reserved *before* `new_window`'s await so a second reconcile
    /// pass landing mid-open sees the id as taken. Without the reservation a
    /// note gets two windows.
    ///
    /// The handle is **weak**. `DesktopContext` is an `Rc<DesktopService>` and
    /// `DesktopService` owns the tao `Window`; dioxus-desktop drops the webview
    /// from its own map when a window closes, but a strong `Rc` held here would
    /// keep the OS window alive after its `VirtualDom` is gone — a visible
    /// window that is never polled again. The crate ships `WeakDesktopContext`
    /// for exactly this (see its doc comment: "the tao window is never dropped
    /// and therefore cannot be closed").
    pub ctx: Option<WeakDesktopContext>,
}

/// Maps note id -> its window, so a note cannot be opened twice, can be closed
/// programmatically when archived, and can be scattered around its neighbours.
pub type StickyRegistry = Signal<HashMap<String, StickySlot>>;

/// Open one note's window at an already-chosen position.
///
/// Assumes the caller has already reserved `note.id` in the registry, and
/// computed `pos` at that same moment — see `setup_sticky_windows` for why the
/// two cannot be separated.
#[allow(clippy::too_many_arguments)]
async fn open_note_window(
    window: DesktopContext,
    mut registry: StickyRegistry,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    passes: Coroutine<PipelineRequest>,
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
        // Rounded corners need a transparent window surface, not just a
        // border-radius: without it the webview paints an opaque sheet and the
        // corners show through as square nubs. Pairs with the (0,0,0,0)
        // background color below — same combination the pill window uses.
        .with_transparent(true)
        .with_inner_size(LogicalSize::new(w as f64, h as f64))
        // A floor, so the grip cannot drag a note down to nothing it can
        // never be dragged back out of.
        .with_min_inner_size(LogicalSize::new(MIN_NOTE_SIZE.0 as f64, MIN_NOTE_SIZE.1 as f64))
        // Honored natively on Windows; ignored by Mutter, which is why the
        // GNOME extension exists. Set on both so the Windows build needs no
        // special case and gets the same scatter for free.
        .with_position(LogicalPosition::new(f64::from(pos.0), f64::from(pos.1)));
    // Windows only, deliberately: this reverses an earlier ruling in
    // agent_docs/sticky_notes.md that kept notes in the taskbar, on the
    // reasoning that a note is not always-on-top and would otherwise have no
    // way back once buried under other windows. That reasoning held until the
    // notes board shipped: it is now the way back to any note, buried or not,
    // so the owner asked for the taskbar to be cleaned up instead. Splash and
    // the recording pill already do this; notes did not because they are the
    // one sticky window kind meant to sit on the desktop like paper, not pop
    // to the front on demand.
    #[cfg(target_os = "windows")]
    {
        builder = builder.with_skip_taskbar(true);
        // tao defaults undecorated windows to a shadowed native drop-shadow
        // (`decoration_shadow`), which insets the client rect by a few
        // DPI-scaled pixels around the whole border. On a note that paints
        // its own rounded shape in CSS, that reserved margin reads as a
        // stray rectangular border. Off, this is a pure CSS-drawn window.
        builder = builder.with_undecorated_shadow(false);
    }

    let cfg = DesktopConfig::new()
        .with_data_directory(super::webview_data_dir())
        .with_window(builder)
        // Clears the webview's own opaque backdrop. `with_transparent` alone
        // only makes the *native* surface transparent.
        .with_background_color((0, 0, 0, 0))
        // Fonts first: STICKY_CSS names "Recursive" and "DM Mono", and an
        // inline <style> carries no @font-face, so without this the note
        // renders in a system fallback while the rest of the app does not.
        .with_custom_head(format!(
            "<style>{}{}</style>",
            crate::ui::fonts::embedded_font_css(),
            STICKY_CSS
        ))
        // Load-bearing for a tray app: without it, archiving the last sticky
        // while the main window is hidden would exit Beamer entirely.
        .with_exits_when_last_window_closes(false);

    let dom = VirtualDom::new_with_props(
        StickyNote,
        StickyNoteProps { id: note.id.clone(), notes, tasks, passes },
    );

    let ctx: DesktopContext = window.new_window(dom, cfg).await;
    tracing::info!("Opened sticky window for note {} at {:?}", note.id, pos);
    registry.write().insert(
        note.id.clone(),
        StickySlot { pos, ctx: Some(Rc::downgrade(&ctx)) },
    );

    // Mutter ignores the position the window asked for, so ask the shell to
    // move it instead. `new_window` resolves when the webview is built, not
    // when the window is mapped and titled, so `place` retries rather than
    // assuming the window is findable yet — and then verifies, because Mutter
    // places the window itself once it is shown and will otherwise undo this.
    #[cfg(target_os = "linux")]
    crate::ui::shell_window::place(title, pos.0, pos.1, all_workspaces);
    #[cfg(not(target_os = "linux"))]
    let _ = (title, all_workspaces);
}

/// Close a note's window and drop its registry slot.
///
/// A weak handle that fails to upgrade means the window is already gone — the
/// user closed it, or the compositor did. That is not an error; the slot is
/// dropped either way so the reconciler can reopen the note if it should be
/// showing.
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
    /// An entry whose window is gone. Reached when a window dies without a
    /// close event — a force-kill, or a compositor crash.
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

/// Decide what a board click means, given the registry and the note.
///
/// Split out from `reopen_note` because the gesture itself needs a live event
/// loop and a real window to test, while this — the part that can actually be
/// wrong — needs neither.
pub fn reopen_action(slot: SlotState, open: bool) -> ReopenAction {
    match (slot, open) {
        (SlotState::Live, true) => ReopenAction::Focus,
        // A live slot on a note already marked closed is a window mid-teardown:
        // its close handler has run but the reconciler has not caught up yet.
        // Focusing it would raise a window that is about to vanish.
        (SlotState::Live, false) => ReopenAction::PruneAndReopen,
        (SlotState::Opening, _) => ReopenAction::Nothing,
        (SlotState::Dead, _) => ReopenAction::PruneAndReopen,
        (SlotState::Absent, _) => ReopenAction::Reopen,
    }
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

/// Bring a note back — the notes board's click handler.
///
/// Pruning a dead slot belongs here rather than in the reconciler. The
/// reconciler must leave stale slots alone or it would fight a user who just
/// closed a window; a board click is the one gesture that unambiguously means
/// "bring it back", so it is the right place to clean up after a window that
/// died without saying so.
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

/// Watch the note store and keep the set of open windows matching it.
///
/// Call once from `App()`. Returns the registry so the notes board can reach
/// the same live windows this effect manages.
pub fn setup_sticky_windows(
    window: DesktopContext,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    passes: Coroutine<PipelineRequest>,
    config: Signal<Config>,
    app_ready: Signal<bool>,
) -> StickyRegistry {
    let registry: StickyRegistry = use_signal(HashMap::new);

    use_effect(move || {
        // Read unconditionally (not just inside the branch below) so the
        // effect is subscribed to it and reruns the moment the splash closes.
        let ready = *app_ready.read();

        // Subscribe to the store — this is the trigger. The registry is only
        // ever `peek`ed, so opening a window does not re-run this effect.
        let store = notes.read();

        let mut to_open: Vec<Note> = Vec::new();
        let mut to_close: Vec<String> = Vec::new();
        {
            // Dead weak handles are deliberately NOT pruned here. Pruning would
            // make the reconciler instantly reopen a window the user had just
            // closed — it would fight them. The correct response to a
            // user-closed window is `set_open(false)`, which `StickyNote`'s own
            // close handler does; this effect then sees `!showing && registered`
            // and takes the branch below. A window that dies *without* a close
            // event (force-kill) leaves a stale slot, and `reopen_note` prunes
            // it at the one gesture that unambiguously means "bring it back".
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

        // Notes restored from disk wait for the loading screen to finish
        // before their windows appear; a note dictated just now can only
        // happen after that point anyway, since the hotkey isn't live yet.
        if to_open.is_empty() || !ready {
            return;
        }

        let area = work_area(&window);
        // Notes stay ordinary windows in the normal stacking order — that
        // decision has not changed. Keeping them away from where the main
        // window sits is the whole reason it does not need to: a note that is
        // not underneath it cannot be hidden by it.
        let main_window = main_window_points(&window);
        // Fresh every pass, so a restart genuinely re-scatters rather than
        // reproducing the previous session's layout.
        let launch = launch_seed();
        // `peek`, not `read`: this is opened-window policy, not a trigger. The
        // effect must re-run when notes change, never when the config does.
        let all_workspaces = config.peek().notes.all_workspaces;

        // Every slot is reserved here, synchronously, before the loop below
        // awaits anything. On restart the reconciler opens every note in one
        // pass; if placement happened after an await, all of them would see
        // the same empty occupied set and stack in one spot. Reserving each
        // slot with its position means the next iteration scatters around the
        // ones already claimed, and it has to happen up front regardless of
        // how the opens themselves are scheduled below.
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

        // Opened one at a time, in one spawned task, rather than one `spawn`
        // per note. On Windows each open drives
        // `CreateCoreWebView2EnvironmentWithOptions` through
        // `wait_with_pump`, a nested Win32 message pump on the main thread,
        // and dioxus drains every pending webview in a single event-loop
        // iteration. That combination is the shape behind dioxus#2483
        // ("Opening Multiple Windows on Desktop-Windows Fails 9 out of 10
        // Times", `WebView2Error(HRESULT(0x8007139F))`). Do not turn this
        // back into a fan-out of one `spawn` per note.
        let window = window.clone();
        spawn(async move {
            for (note, pos) in pending {
                open_note_window(
                    window.clone(),
                    registry,
                    notes,
                    tasks,
                    passes,
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
        // Without the prune the slot still counts as registered, the
        // reconciler never opens a replacement, and the board click no-ops
        // forever with nothing to show for it.
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
    fn an_open_already_in_flight_is_left_alone() {
        // Pruning a reserved slot would let the reconciler start a second
        // window while the first is still being constructed.
        assert_eq!(reopen_action(SlotState::Opening, false), ReopenAction::Nothing);
        assert_eq!(reopen_action(SlotState::Opening, true), ReopenAction::Nothing);
    }
}

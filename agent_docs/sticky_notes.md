# Sticky Notes

A second global hotkey dictates into a **sticky note** on the desktop instead of
injecting into the focused field. Phase 1 — capture, persist, place, find — is
complete. Phases 2 (cleanup pass) and 3 (task extraction) are not built.

- Spec: `docs/superpowers/specs/2026-08-20-sticky-notes-design.md`
- Plan: `docs/superpowers/plans/2026-08-20-sticky-notes-phase1.md`

## Read this first: extensions do not hot-reload on Wayland

Every change to `extension/beamer-focus@beamer.app/` needs a **full GNOME log
out and back in**. There is no reload, no `--replace`, no D-Bus call that
avoids it. Skipping it means testing stale code and drawing false conclusions
from it — the shell keeps serving the version it loaded at login even after the
files on disk have changed, and `gnome-extensions list --details` reports that
cached version too, not the one in the directory.

Check what is actually live:

```bash
gdbus call --session --dest org.gnome.Shell \
  --object-path /app/beamer/FocusProvider \
  --method app.beamer.FocusProvider.GetVersion
```

Batch extension changes and sequence the log out last. Every Rust caller of a
newer method degrades to a silent no-op against an older helper, so a
half-updated system is safe to keep working in.

## Why the extension exists at all

Sticky notes need to be somewhere sensible on screen. On Wayland an ordinary
client cannot put itself there:

- **No protocol exists.** Core Wayland and `xdg-shell` give clients no way to
  learn or set absolute position. The compositor owns placement, by design.
- **`tao::Window::outer_position()` lies.** It returns `Ok((0, 0))` under
  Wayland rather than an error — it reads a cached atomic fed by GDK
  `frame_extents()` on `configure_event`, and GDK has no global coordinates
  there. Never call it on Wayland; believing it persists garbage silently.
- **`xx-session-management-v1`** is the eventual portable answer, and its design
  confirms the rule: the app never learns coordinates, it names a window and
  asks the compositor to restore it. Not exposed by Mutter on GNOME 50.1.

Code inside GNOME Shell is not a Wayland client and is bound by none of this.
Beamer already ships an extension for text injection and the recording pill, so
placement costs two D-Bus methods rather than a new dependency.

## The title contract

The extension matches note windows by **exact title**: `Beamer Note <id>`.

- Produced by `ui::sticky::window_title` (`TITLE_PREFIX` + note id).
- Consumed by `_findWindowByTitle` in `extension.js`.
- Pinned by a test in `ui::sticky`.

Titles are not owned — any window may claim one — so `_findWindowByTitle`
prefers a match whose app id looks like Beamer's and falls back to a title-only
match, logging when it does. A hard filter was rejected: guessing the app id
wrong would break placement silently, and diagnosing that costs a log out.

## Notes are placed, not remembered

**Position persistence was dropped by decision (2026-08-21).** Nothing records
where a note was; `ui::note_layout::place_next` scatters each new window around
the ones already on screen, freshly, every launch. Restart with several notes
open and they reappear in *different* places. That is correct, not a bug.

This removed the hardest and least reliable part of the original design —
reading a window's own geometry back, and the close-race that came with it —
and replaced it with a pure function that has real tests.

`Note::pos` still exists because it is part of the persisted schema and
`with_position` is honoured natively on Windows. **Nothing writes it from window
geometry, and nothing should.**

Placement lives in two halves:

| Module | Job |
|---|---|
| `ui::note_layout` | Pure. Best-candidate sampling over a deterministic LCG. No I/O, no Dioxus. |
| `ui::shell_window` | D-Bus client for `PlaceWindow`. Retries ~2s because Beamer cannot observe the Wayland map event. |

Two ordering facts that are easy to get wrong:

- **Position is chosen at slot-reservation time, synchronously**, not inside
  `open_note_window`. On restart the reconciler opens every note in one pass; if
  placement happened after the `await`, all of them would see the same empty
  occupied set and stack in one spot.
- **`MonitorHandle::size()` is physical pixels; `move_frame` is logical stage
  coordinates.** Identical at scale 1.0, divergent at any other.

## The reconciler and its invariant

Windows are not opened by whoever creates a note. One effect in
`ui::sticky_windows` watches the store and makes the live window set match:

```text
open && !archived && not registered  ->  open a window
(archived || !open) && registered    ->  close it
```

One mechanism covers both a note dictated just now and a note loaded from
`notes.json` at startup.

> **Invariant:** the effect **reads** `notes`, **peeks** the registry, and
> **writes only the registry**. Every write to `notes` happens outside it.

A write to `notes` from inside its own trigger is an unconditional re-trigger
loop; a `read()` of the registry would make opening a window re-run the effect
that opened it.

Dead registry slots are deliberately **not** pruned by the reconciler. Pruning
would instantly reopen a window the user just closed — the reconciler would
fight them. `sticky_windows::reopen_note` prunes instead, at the one gesture
that unambiguously means "bring it back".

## Why the close handler lives in `StickyNote`, not `App()`

**This is the least re-derivable fact in the feature.**

`use_wry_event_handler` handlers are **per-window**. `create_wry_event_handler`
keys the handler to the window that registers it
(`desktop_context.rs:233` — `self.shared.event_handlers.add(self.window.id(), ...)`),
and `apply_event` skips any `Event::WindowEvent` whose `window_id` differs
(`event_handlers.rs`, the `continue`).

A close handler registered in `App()` would therefore only ever see the **main
window's** events. It would compile, run, and never fire — a silent no-op that
reads as entirely correct. Registered inside `StickyNote`, `window()` resolves
to that sticky's own `DesktopContext` (each webview gets one via
`webview.rs:519`), so it sees its own close and nothing else. No `WindowId` map
is needed.

Two supporting facts, both verified in dioxus-desktop 0.7.9's source:

- `App::tick()` runs `apply_event` **before** the match that dispatches
  `WindowEvent::CloseRequested`, so the handler fires while the webview still
  exists and its write to the store lands.
- A programmatic `ctx.close()` sends `UserEvent(CloseWindow)` straight to
  `handle_close_requested` and **never** produces a `WindowEvent`. So the
  `CloseRequested` arm fires only for user and compositor closes, the archive
  path cannot double-fire through it, and no guard flag is needed.

The hook is registered **above** the component's early return. A hook after a
conditional return breaks hook ordering the moment the note is archived.

## Cross-window state

Each sticky window is its own `VirtualDom` with its own scope tree.

- **`use_context` does not cross the boundary** — it walks only the current
  dom's tree, so a provider in `App()` is invisible from a sticky. The store is
  passed as a **prop** via `VirtualDom::new_with_props`.
- **`GlobalSignal` is worse, not better.** `get_global_context()` resolves
  through `Runtime::current()` and stores its map on *that dom's* root scope, so
  every window silently gets its own independent copy. No error, no panic,
  values just diverge.
- **`Signal<NoteStore>` is not `Sync`** — it cannot be stashed in a
  `static OnceLock`. Thread it as a parameter.
- **Never move a `Signal` into `tokio::spawn`.** Desktop's tokio runtime is
  multi-threaded and generational-box's arena is thread-local. Use dioxus's own
  same-thread `spawn`.

## Persistence

`notes.json` beside `config.toml`. Written atomically (temp file + rename); a
corrupt file is preserved as `.json.corrupt` rather than overwritten.

Three write paths, because one is not enough:

1. **Immediate flush on capture** (`orchestrator::sink::do_note_capture`) — a
   just-captured transcript must never be lost to a crash.
2. **500 ms debounce tick** (`app_setup::setup_notes_flush`) — body edits are
   per-keystroke. It gates on `is_dirty()` via `peek()` before taking `write()`;
   an unconditional write per tick would notify every open sticky window twice a
   second.
3. **Flush in the tray Quit handler** — `process::exit` skips destructors *and*
   the interval tick.

## Local AI

`src/llm/` is a client of a **standalone** `llama-server`. Beamer never spawns
it. See `agent_docs/config_schema.md` and `deploy/`.

⚠️ **Never poll `GET /v1/models` on a timer.** A status read resets the server's
per-model idle clock, so a background health check pins the ~3 GB extraction
model in VRAM permanently — no error, no symptom. On button press and once when
the settings page opens, nowhere else.

⚠️ `MODEL_CREDIT` in `src/llm/mod.rs` is a **licence condition**. `s1-mini` is
Apache 2.0 plus a binding term requiring the exact string
`"S1-mini" by "Superwhisper"`. Pinned by an exact-equality test.

## Gotchas

- **`note_hotkey = ""` means note capture is off entirely**, by design, so the
  dictation hotkey can never be silently diverted. Set one before testing.
- Notes are **not** always-on-top, deliberately. They sit in the normal
  stacking order.
- `with_exits_when_last_window_closes(false)` on sticky windows is
  load-bearing: without it, archiving the last note while the main window is
  hidden would exit Beamer entirely.
- The registry holds **weak** handles. A strong `Rc` would keep the OS window
  alive after its `VirtualDom` is gone — a visible window that is never polled.

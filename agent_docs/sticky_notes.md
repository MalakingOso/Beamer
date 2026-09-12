# Sticky Notes

A second global hotkey dictates into a **sticky note** on the desktop instead of
injecting into the focused field. All three phases are built: capture, persist,
place and find (1); an S1-mini cleanup pass (2); and task suggestions the user
accepts or dismisses (3).

This document is about the **windows** — placement, cross-window state, and the
things about Wayland that look broken and are not. The two model passes have
their own document.

- Model passes: **`agent_docs/local_inference.md`** — read it before touching
  `src/llm/` or `src/notes/pipeline.rs`.
- Design history: `docs/decisions.md` ("Sticky notes, all three phases")

## Note state is two fields, not one

`NoteState` is gone. A note carries `clean_state` and `extract_state`, each a
`StageState { Pending, Done, Failed, Skipped }`, because a note can legitimately
be cleanup-failed *and* extraction-succeeded at once — a failed cleanup is meant
to leave extraction to run against `raw`. One linear enum could not say that,
and a successful extraction would erase the record a retry affordance keys off.

`Skipped` is not `Pending`. `Pending` has something to retry; `Skipped` means
the user turned the pass off, and the footer offers nothing for it.

Migration was free and must stay free: `Note` has no `deny_unknown_fields`, so
an older `notes.json` carrying `"state": "raw"` loads with the stale key ignored
and both new fields defaulting to `Pending`.

`NoteOrigin` records whether a note was dictated or typed. It is passed
explicitly to `create` rather than defaulted, because it is corpus provenance
and a silent default is exactly what corrupts a corpus.

⚠️ **Only `notes/lifecycle.rs` writes a stage result.** Its methods are machine
writes: none bump `modified` (that is user-facing ordering) and none touch
`raw`. `apply_cleanup` is a compare-and-swap on the body captured at send time —
**body equality, not a timestamp**, because `set_color` bumps `modified` for
something that is not an edit. `set_open` used to be in that list too; since
Task 7 it is machine-local and does not touch `modified` at all. See
"Machine-local state" below.

## The footer: three icons, and a fading failure

`src/ui/sticky_footer.rs`'s `footer()` decides the glyph from three inputs, not
two: `clean_state`, `extract_state`, and now whether a pass for this note is
in flight *right now*. In flight outranks everything, including a stale
`Failed` from a prior attempt — a retry actually running must never still show
the last attempt's failure text.

"In flight" is **not** `StageState` — there is deliberately no `Running`
variant, since `StageState` is persisted into the automerge doc and a
transient runtime fact has no business there. Instead `pipeline.rs`'s
`use_pipeline` keeps two copies of the same membership: the coroutine's own
`Rc<RefCell<HashSet<String>>>` (a synchronous dedup guard, never a `Signal`)
and a `Signal<HashSet<String>>` mirror, updated at the same insert/remove call
sites, that exists purely so `StickyNote` can react to it. That signal is
threaded down through `setup_sticky_windows` / `StickyNoteProps` alongside
`notes` and `passes`.

The failed-state red text (`.sticky-pass-error`) auto-hides itself ~7s after
appearing, in `sticky.rs`, independent of whether the sweep has actually
retried the note yet — this is a presentational fix, not a retry-policy
change. It cannot key off `note.clean_state`/`extract_state` directly: those
fields are backed by a `use_memo` that dedups by `Note`'s `PartialEq`, so a
retry that fails the *same way twice in a row* produces an identical `Note`
and never triggers a re-render. The timer instead watches the in-flight
signal's own true→false transition for this note (a plain `Signal` write is
never deduped), and checks the resulting state only at that edge. A note
opened already-`Failed` never crosses that edge in its window, so mount is a
second trigger: it gets one timed display too, or the text would show
forever. Both cases are one pure predicate,
`sticky_footer::should_flash_error`, pinned by tests there. Once the
text hides, the asterisk and its retry affordance stay exactly as before —
only the words go quiet.

`apply_cleanup`'s compare-and-swap (`notes/lifecycle.rs`) also resets a stale
`Failed` back to `Pending` on `Superseded`: a retry response landing after a
mid-flight edit is not evidence of anything actually broken, and leaving
`Failed` in place would make the footer lie indefinitely.

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

## A note body is one textarea of plain text

A note holds text: `body` is one `String`, rendered as one `<textarea>` sized
by `rows_for` (`ui::sticky`). There is no attachment concept anymore — file
and image attaching (`notes::blocks`, the `[[beamer:<id>]]` tokens,
`Attachment`/`Location`, `ui::sticky_blocks` and its `/note-media` asset
handler, the paperclip, drag-drop and link chips) was removed outright, with
the document's old per-note `attachments` map deleted on reconcile and the
`attachments` key in pre-removal `notes.json` files ignored on load. A URL
typed or dictated into a note stays plain text; linkifying inside a
`<textarea>` is not possible without replacing the editor.

⚠️ **`raw` stays the verbatim transcript.** Cleanup may rewrite `body`, never
`raw`.

## Notes are resized by the client, and the size is remembered

`with_decorations(false)` means the compositor offers no edge to grab, so
`.sticky-grip` in the footer calls `drag_resize_window(ResizeDirection::SouthEast)`
— tao's wrapper for `xdg_toplevel.resize`. Unlike positioning this is
**client-initiated**, so it needs no extension method and **no log out**.
`with_min_inner_size` gives a 180×140 floor so a note cannot be dragged to
nothing.

Size is captured where position deliberately is not, and the reason is
principled rather than pragmatic: **a window's size arrives in the compositor's
configure event**, whereas its position is something a Wayland client is never
told. `WindowEvent::Resized` is a fact; `outer_position()` is a lie that returns
`Ok((0,0))`.

Two things to get right, both otherwise silent:

- ⚠️ **`Resized` carries `PhysicalSize`; the builder consumed a `LogicalSize`.**
  Divide by `scale_factor()` and store logical. At scale 1.0 the two agree, so
  the bug is invisible on this machine and reopens every note at double size on
  a HiDPI one. `ui::sticky::logical_size` does it, and is tested.
- ⚠️ **Guard with `peek` *before* any `write()`.** `Resized` fires once per
  frame of a grip drag and again when the window maps, and `Signal::write`
  notifies every subscriber whether or not the value changed — so an unguarded
  call re-renders the note and the whole board for a size already recorded.
  `set_size`'s own guard is not enough: by then the lock is taken. `set_size`
  also does **not** bump `modified`, or the board would reshuffle per mouse
  move. Since Task 7 it writes to `machine.json`, not `notes.json`, so a
  resize does not dirty the synced note at all. See "Machine-local state" below.

This does **not** reopen the decision below. Notes still appear somewhere new
each launch, now at the size you left them — and `place_next` already takes a
size, so the scatter improves for free.

## Notes are placed, not remembered

**Position persistence was dropped by decision (2026-08-21).** Nothing records
where a note was; `ui::note_layout::place_next` scatters each new window around
the ones already on screen, freshly, every launch. Restart with several notes
open and they reappear in *different* places. That is correct, not a bug.

⚠️ **The scatter is seeded by the caller, and that is deliberate (2026-08-25).**
`place_next` takes a `seed` so it stays a pure function the tests can pin, while
`ui::sticky_windows` supplies a fresh one per launch from the wall clock and
mixes in each note's index. Both halves matter: an earlier version derived the
seed from `occupied.len()` alone, which was pure and tested and *guaranteed the
Nth note landed on the same pixel every launch* — the exact opposite of what
this section promises. Drop the index mixing and every note in one restore draws
the same candidate points instead, so they pile up.

The scatter also spans **every** monitor and avoids the main window's estimated
rectangle. Notes are still not always-on-top; keeping them out from under the
main window is what makes that decision survivable, not raising them above it.
The main window's rectangle is an *estimate* — centre of the first monitor, at
its build size — because it is created with no position and Beamer cannot read
where Mutter put it: `outer_position()` lies under Wayland, and the title lookup
is ambiguous since the splash window is titled "Beamer" too. Being wrong only
scatters notes around a patch of empty desktop.

This removed the hardest and least reliable part of the original design —
reading a window's own geometry back, and the close-race that came with it —
and replaced it with a pure function that has real tests.

`pos` still exists (`NoteStore::pos`/`set_pos`, backed by `MachineStore`) because
it is part of the persisted schema and `with_position` is honoured natively on
Windows. **Nothing writes it from window geometry, and nothing should.** It moved
off `Note` itself in Task 7; see "Machine-local state" below.

Placement lives in two halves:

| Module | Job |
|---|---|
| `ui::note_layout` | Pure. Best-candidate sampling over a deterministic LCG, seeded by the caller. No I/O, no Dioxus. |
| `ui::shell_window` | D-Bus client for `PlaceWindow`. Retries ~5s and **verifies the result**, because neither the map event nor the placement can be observed directly. |

Four facts that are easy to get wrong, three of them measured on 2026-08-25
against a two-monitor GNOME 50 Wayland session:

- **Position is chosen at slot-reservation time, synchronously**, not inside
  `open_note_window`. On restart the reconciler opens every note in one pass; if
  placement happened after the `await`, all of them would see the same empty
  occupied set and stack in one spot.
- **`MonitorHandle::size()` is physical pixels; `move_frame` is logical stage
  coordinates.** Identical at scale 1.0, divergent at any other. Measured here:
  two monitors reporting 5120x2880 physical at x=0 and x=5120, scale 2 — so the
  logical desktop is 5120x1440 and the second monitor's origin is 2560, not
  5120. Divide by **each monitor's own** scale; there is no single factor on a
  mixed-DPI desktop.
- ⚠️ **`window.primary_monitor()` returns `None` on this session, while
  `available_monitors()` enumerates both screens correctly.** This is the normal
  path, not an edge case. `work_area` used to ask only for the primary monitor
  and therefore fell through to its hardcoded 1920x1080 fallback on every single
  launch, confining every note to the top-left seventh of the desktop. Anything
  that needs a monitor must fall back to `available_monitors().next()`.
  `ui::app_setup`'s splash centring still has the unfixed version of this.
- ⚠️ **`PlaceWindow` returning `true` does not mean the window stayed put.** See
  below — this is the one that cost the most time.

### The clobber: a placement that succeeds and is then undone

`PlaceWindow` returns `true` when it *found a window by that title and called
`move_frame` on it*. Mutter applies its **own** initial placement when a window
is first shown, and that happens **after** the window is already findable by
title. So an early call is accepted, logged as a success, and silently
overwritten.

Measured 2026-08-25, three notes, ~10ms after `new_window` resolved:

| Asked for | Actually ended up at |
|---|---|
| (2311, 508) | (1120, 590) |
| (3389, 1036) | (1146, 616) |
| (1648, 584) | (1196, 666) |

That 50px cascade is Mutter's default placement — and it is *exactly* the
clustering the scatter exists to prevent. Re-issuing the identical call against
the settled window moved it correctly on the first try, so `move_frame` was
never at fault. Believing the first `true` was.

`place_blocking` therefore does not stop at `true`. It sleeps, re-reads the
frame with `GetWindowFrame`, and only believes a placement that is **still
there**. The sleep is the load-bearing part: a read taken immediately confirms a
position Mutter has not clobbered *yet*. `SETTLE` (500ms) is the floor before
any read is trusted, chosen against the timeline above.

⚠️ **Never treat a bare `(true,)` from `PlaceWindow` as verification, in code or
at the command line.** The only proof is a `GetWindowFrame` read taken after the
window has settled. An earlier version of this document recorded `PlaceWindow`
as "verified to actually move a window" on the strength of a manual `gdbus`
call — that call was made against a window that had been open for minutes, so it
tested the one case that was never broken.

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

Migration is free and stays free: `Note` has no `deny_unknown_fields` and every
field added since v1 is `#[serde(default)]`. A `notes.json` written while
attachments existed loads with its text intact and the stale `attachments` key
ignored (and dropped on the next flush).

Three write paths, because one is not enough:

1. **Immediate flush on capture** (`orchestrator::sink::do_note_capture`) — a
   just-captured transcript must never be lost to a crash.
2. **500 ms debounce tick** (`app_setup::setup_notes_flush`) — body edits are
   per-keystroke. It gates on `is_dirty()` via `peek()` before taking `write()`;
   an unconditional write per tick would notify every open sticky window twice a
   second.
3. **Flush in the tray Quit handler** — `process::exit` skips destructors *and*
   the interval tick.

Discrete gestures (new note, archive, delete) also flush **inline**, on the
same argument as capture: a crash before the tick must not resurrect them.

### Machine-local state: pos, size, open (Task 7)

`pos`, `size` and `open` are no longer fields on `Note`. They moved to
`machine.json` beside `notes.json`, keyed by note id, owned by
`notes::machine::MachineStore` and reachable through the same `NoteStore`
method names call sites already used (`set_size`, `set_open`, and the new
getters `size`, `is_open`, `pos`/`set_pos`).

The reason is sync, even though sync itself is a later phase. `pos`, `size` and
`open` describe a window on one desktop, not a note's content, and `set_open`
used to call `touch()`, rewriting `modified` for something that is not an
edit. Under any last-write-wins merge, closing a sticky on one machine would
make that note look newer than a real body edit made on another and win a
merge it had no business winning. `set_open` no longer touches `modified` at
all, and `notes.json` no longer carries these fields, so there is nothing left
for a merge to see. `set_size` had the milder version of the same problem
(resizing generated sync churn with no content change) and is fixed the same
way.

`machine.json` also carries `machine_id`: sixteen hex digits, generated once per
install and never synced. Note ids need it because the old id shape,
`{millis:x}-{counter:04x}`, restarts its counter at 0 every process, so the
first note of every session was `…-0000`. Two machines creating their first
note in the same millisecond would produce the same id, and `Task.note_id` is
a foreign key into that namespace, so a collision would silently reparent tasks
onto the wrong note. `notes::next_note_id` mints note ids as
`{millis:x}-{counter:04x}-{machine}` instead, and `next_synced_id` mints task
ids the same way. Existing ids keep working, they are opaque strings and
nothing parses them, on either side.

Migration is one-way and, once it has run, self-erasing. `NoteStore::load`
re-parses `notes.json` a second time into a throwaway shape that still
declares `pos`/`size`/`open`, lifts any it finds into `machine.json` (an id
`machine.json` already has an entry for is left alone, so a second migration
pass can't clobber real window state with a stale file's numbers), and GCs
`machine.json` down to the ids `notes.json` actually has. `Note`'s own
deserialize just ignores the stale keys (no `deny_unknown_fields`, and that
has to stay true), so after the first save `notes.json` stops carrying them at
all and the second parse finds nothing to lift.

Nothing in `ui::sticky_windows` reads `note.pos` even on Windows: it never did.
`with_position` there is built from the placement `note_layout::place_next`
computes fresh each launch, not from a stored value. `pos`/`set_pos` exist on
`NoteStore` and `MachineStore` because the field is part of the persisted
schema (see "Notes are placed, not remembered" above), not because anything
calls them today. Both carry `#[allow(dead_code)]` for exactly that reason,
the same status the field had before this task.

### Delete, which the store did not have until now

`archive` remains the everyday, non-destructive gesture. `NoteStore::delete`
removes a note outright and is offered **only on an archived card, behind a
two-step confirm** on the board. It also drops the note's `machine.json` entry
immediately, rather than waiting for the GC pass on the next load.

Deleting a note also drops its rows from `TaskStore` (`delete_for_note`). That
is a **deliberate exception** to "dismissed rows are retained as labelled
negatives" — everywhere else a decision is permanent corpus data, but an
explicit delete means gone, and keeping the rows would leave the corpus holding
labels for a note whose text no longer exists to explain them. Both mutations
happen in memory before a single `flush_stores` call; a crash between the two
mirror writes inside that call can still strand rows, so `flush_stores` prunes
rows whose note is gone on the next pass (never over a failed load, where
"gone" means "unreadable").

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

## Manual QA checklist

Run this after any change that touches capture, placement or the reconciler.
⚠️ Turn on "Note capture" in Settings → Recording first — an empty
`note_hotkey` means it is off by design, so the dictation hotkey can never be
silently diverted.

1. Dictation hotkey → text still injects as before. *(The regression that
   matters most — note capture must never steal the dictation binding.)*
2. Note hotkey → sticky appears, pill shows the purple note ring, waveform
   still tracks the mic.
3. Dictate four or five notes → spread irregularly, none stacked, none
   off-screen.
4. Close one with Alt+F4 → `machine.json` shows `"open": false` for that
   note's id.
5. Notes board: search finds a note by a word actually *said*; clicking a
   card reopens it; archive hides it; "Show archived" restores.
6. Restart with several notes open → they reappear, freshly scattered.
   Positions deliberately do **not** match the previous session.
7. Local AI card with the server up → lists both models and their states.
   Then `pkill -x llama-server` (**never** `pkill -f`, which matches the shell
   running it) → card reports not running, notes still captured.

## Gotchas

- **The note UI needs no extension change, and no log out.** The sticky body,
  the resize grip and delete are all client-side. `GetVersion` is still the
  only authoritative way to check the running version.

- **The extension's version number is its deploy trigger, not just its D-Bus
  contract.** `status()` in `src/install/gnome_extension.rs` calls `install()`
  — the copy into `~/.local/share/gnome-shell/extensions/` — only when
  `live < bundled`. So a change to any `.js` file here that adds no method
  ships *nothing* unless you also bump `HELPER_VERSION` in `extension.js` and
  `"version"` in `metadata.json`, together. This bit once: 7f83eeb's pill
  waveform change sat undeployed through a full log out, and the behaviour
  being evaluated was the Aug-21 code. v6 (2026-08-24) is that bump.
  Cross-check the installed copy, not just the repo:
  `ls -l ~/.local/share/gnome-shell/extensions/beamer-focus@beamer.app/`.

- **`note_hotkey = ""` means note capture is off entirely**, by design, so the
  dictation hotkey can never be silently diverted. Set one before testing —
  Settings → Recording → "Note capture" flips it on and proposes
  `Ctrl+Alt+Space`. Switching it off writes `""` back rather than remembering
  the chord, because `""` is the only value the hotkey layer reads as unbound.
- **The note chord must not be a prefix of the dictation chord.**
  `matching_binding` compares the modifier set held *at the instant the trigger
  goes down*, so with dictation on `Ctrl+Super` (trigger `VK_LWIN`), a note
  chord of `Ctrl+Super+Space` fires dictation at Super-down — before Space is
  ever pressed. Compounding that, `HotkeyConfig` has no Meta modifier field at
  all, so `Ctrl+Super+Space` parses to plain `Ctrl+Space` anyway (see todo.md).
  `Ctrl+Alt+Space` is the default precisely because it shares no trigger with
  `Ctrl+Super`. Pinned by `recording_card::tests`.
- **One string format, two parsers.** `settings/hotkey_picker.rs` renders and
  reassembles chords; `HotkeyConfig::parse` registers them. Nothing in the type
  system keeps them in step, so
  `hotkey_picker::tests::what_the_picker_emits_is_what_the_engine_registers`
  pins the round trip. Both dictation and note rows share the one picker
  component so the two chords can never diverge in interpretation.
- **The title bar IS the drag handle.** Notes are built with
  `with_decorations(false)`, so there is no compositor titlebar to grab and
  nothing moves them. `.sticky-bar` calls `window.drag()` on `onmousedown`,
  which is tao's `drag_window()` → `xdg_toplevel.move` under Mutter: the client
  asks the compositor to take over an interactive move. This is the *only*
  client-side way to move a Wayland window, and it does not contradict the
  no-position rule above — the app still never learns a coordinate.
  ⚠️ Dioxus discards the `Result` (`_ = self.window.drag_window()`), so a
  failure is **silent**. If a note stops moving, look there before the CSS.
- **Every control inside the bar needs `onmousedown: e.stop_propagation()`.**
  Otherwise the bar's mousedown starts a window drag and the compositor eats
  the click, so colour swatches and the archive button never fire. The classic
  custom-titlebar bug.
- **Rounded corners need a transparent window, not just `border-radius`.**
  `with_transparent(true)` + `with_background_color((0,0,0,0))` +
  `html,body,#main{background:transparent}`. Any one of the three missing and
  the webview paints an opaque sheet that shows through the corners as square
  nubs. `.sticky` also needs `overflow:hidden` or the bar squares off the top
  two corners again. Radius is 8px — the documented maximum in
  `design_system.md` ("Never rounder").
- **Hard-offset shadows need padding to render into.** `.sticky` fills the
  window, so `box-shadow` was clipped by the window edge and simply never
  appeared. `#main` carries `padding:0 5px 5px 0` as shadow room.
- **Secondary windows do not get the app's fonts for free.** The main window
  `<link>`s `assets/styles.css` and its `@font-face` rules resolve; stickies,
  pill and splash inject CSS with `with_custom_head`, which carries none. Every
  `font-family:"DM Mono"` in those windows fell back to a system font for
  months and looked like a deliberate style. `ui::fonts::embedded_font_css()`
  fixes it with `data:` URIs — chosen over an `asset!()` URL because a URI
  cannot fail to resolve and *can* be unit-tested, where a silent font fallback
  in a webview cannot. **The pill and splash windows still have this bug.**
- Notes are **not** always-on-top, deliberately. They sit in the normal
  stacking order.
- `with_exits_when_last_window_closes(false)` on sticky windows is
  load-bearing: without it, archiving the last note while the main window is
  hidden would exit Beamer entirely.
- The registry holds **weak** handles. A strong `Rc` would keep the OS window
  alive after its `VirtualDom` is gone — a visible window that is never polled.
- **Windows placement now clears the taskbar, confirmed on real hardware.**
  `ui::work_area::work_area` used to union each monitor's full physical
  resolution, which on Windows meant notes could be placed under the
  taskbar. It now reads each monitor's usable rectangle via
  `GetMonitorInfoW`/`MONITORINFO::rcWork` on `#[cfg(target_os = "windows")]`,
  falling back to the full monitor rectangle if that call fails, and cancels
  the GNOME-panel inset back out on that target rather than forking the
  tested union math. Mixed-DPI multi-monitor placement stays wrong on Windows
  even with this fix: each monitor's rectangle is divided by *its own* scale
  factor before the union, and that only produces one consistent logical
  coordinate space when every monitor shares a scale. A single display, or
  several matched ones, is fine; a genuinely mixed-DPI pair is not — that
  specific case is still unconfirmed on a real Windows multi-monitor setup.
- **Note windows stay in the taskbar, on purpose.** They omit
  `with_skip_taskbar(true)` even though the splash and the pill both set it,
  so six open notes means six taskbar buttons. This is a ruling, not an
  oversight: notes are deliberately not always-on-top, so a skip-taskbar note
  buried under other windows would have no way back to it, and Microsoft's own
  Sticky Notes appears in the taskbar too. Do not add `with_skip_taskbar` here
  without revisiting that trade-off first.

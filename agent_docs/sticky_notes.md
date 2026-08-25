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
- Spec: `docs/superpowers/specs/2026-08-20-sticky-notes-design.md`
- Plan: `docs/superpowers/plans/2026-08-20-sticky-notes-phase1.md`

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
**body equality, not a timestamp**, because `set_color` and `set_open` bump
`modified` for things that are not edits.

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

## A note body is a block stack, not a textarea

A note holds text, images, links and dropped files **interleaved in reading
order**. The body is still one `String`; attachments live in it as placeholder
tokens.

```
body:  "Ring Sarah about the\n[[beamer:18f2a-0001]]\nQ3 deck before Friday."
        └── run 0 ──────────┘ └── attachment ─────┘ └── run 1 ───────────┘
```

The grammar is deliberately narrow, and lives in exactly one module,
`notes::blocks` — pure, no Dioxus, fully tested:

- A token is `[[beamer:<id>]]` **alone on its own line**, after trimming.
- Anything else, including a token mid-line, is literal text.
- `[[` cannot come out of dictation, so no transcript can accidentally make one.
- **An id with no matching `Attachment` renders as literal text.** A
  desynchronised note fails visibly rather than swallowing a line.

`parse` returns strictly alternating `Text`/`Attachment` blocks, synthesizing
**empty text runs** at both ends and between adjacent attachments — that is what
gives the UI a textarea above a leading image and below a trailing one.
`reassemble` works off an exact tiling of the original body, so an all-`None`
result is byte-identical; that is what makes "cleanup changed nothing" mean
nothing changed.

⚠️ **`NoteStore::search` matches `blocks::plain_text(body)`, not `body`.**
Without it every attachment-bearing note would match the query "beamer" through
its own tokens.

⚠️ **`raw` never gains a token.** It stays the verbatim transcript.

⚠️ **Tokens never reach either model.** This is the constraint that shapes the
cleanup path — see `agent_docs/local_inference.md`, failure mode 6, before
touching `src/llm/` or `notes/pipeline.rs`.

| Module | Job |
|---|---|
| `notes::blocks` | The grammar. `parse`, `plain_text`, `reassemble`, `set_run`, `insert_token`, `remove_token`. Pure. |
| `notes::edit` | `add_attachment` / `remove_attachment` / `prune_attachments` / `relocate_attachment`, plus `set_size` and `delete`. |
| `ui::sticky_blocks` | Rendering, the per-window asset handler, and drop classification. |

### Files are referenced, never copied

An `Attachment::Image` holds the **path to your file**. Beamer never copies,
moves or deletes it, and deleting a note cannot delete your photo. The price is
that moving the file breaks the reference — made visible, never silent: the
block becomes a muted card naming the file, with the full path on hover and a
**Locate…** button that reopens the picker and repoints it.

Because Beamer owns no media files, the only thing that can be *orphaned* is a
**record** — an `Attachment` whose token the user deleted out of a textarea.
`prune_attachments` collects those, in the same store write as the edit.

### Getting content in

| Gesture | Result |
|---|---|
| Drop a file | `Image` if the extension is a WebKit-decodable raster, else `File` |
| Drop a link from a browser | `Link` chip, from `text/uri-list` or a bare `text/plain` URL |
| Paste a bare URL | `Link` chip |
| Paperclip in the bar | Native file dialog — the fallback, and the Windows path |

Notes:

- **`ondragover` must call `prevent_default()`** or `ondrop` never fires at all.
- On every platform but Windows, wry's native drag-drop handler merges real
  filesystem paths into the HTML event, so `e.files()` is authoritative when it
  is non-empty (`dioxus-desktop/src/webview.rs:157-185`).
- ⚠️ **`ClipboardData` carries nothing on desktop** —
  `SerializedClipboardData` is an empty struct. The clipboard is read directly
  with `arboard`, **synchronously**, because `prevent_default` rides the event's
  own IPC response and a spawned read would answer too late to suppress the
  insert.
- **Pasted image bytes are out of scope**, deliberately: an attachment is a
  reference to a file, and a screenshot on the clipboard is not a file anywhere.
  Detected rather than ignored — the footer says so and points at the paperclip.
- ⚠️ Only **dropped or pasted** links become chips. A URL typed or dictated
  inside a run stays plain text; linkifying inside a `<textarea>` is not
  possible without replacing the editor.
- Dropping *between* two runs is deferred; a drop appends at the end.

### Images are served, not inlined

`img { src: "/note-media/<attachment-id>" }`, answered by an asset handler.

⚠️ **Registered inside `StickyNote`, from `sticky_blocks::use_note_media`** —
`use_asset_handler` resolves `crate::window()` by `consume_context`, so a
registration in `App()` binds to the main window and every note's image 404s,
silently, as a broken image. Same per-window rule as the close handler below.

The URL carries an **id, never a path**: the handler resolves it against that
note's own `attachments` and can therefore only ever serve a file that note
already references. Chosen over `data:` URIs, which would put whole photos into
the DOM string on every render.

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
  also does **not** bump `modified`, or the board would reshuffle per mouse move.

This does **not** reopen the decision below. Notes still appear somewhere new
each launch, now at the size you left them — and `place_next` already takes a
size, so the scatter improves for free.

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

Migration is free and stays free: `Note` has no `deny_unknown_fields` and every
field added since v1 is `#[serde(default)]`. A `notes.json` written before
attachments existed loads with an empty vec.

Three write paths, because one is not enough:

1. **Immediate flush on capture** (`orchestrator::sink::do_note_capture`) — a
   just-captured transcript must never be lost to a crash.
2. **500 ms debounce tick** (`app_setup::setup_notes_flush`) — body edits are
   per-keystroke. It gates on `is_dirty()` via `peek()` before taking `write()`;
   an unconditional write per tick would notify every open sticky window twice a
   second.
3. **Flush in the tray Quit handler** — `process::exit` skips destructors *and*
   the interval tick.

Dropping, pasting and attaching also flush **inline**, on the same argument as
capture: a photo you just dropped must not be lost to a crash before the tick.

### Delete, which the store did not have until now

`archive` remains the everyday, non-destructive gesture. `NoteStore::delete`
removes a note outright and is offered **only on an archived card, behind a
two-step confirm** on the board.

Deleting a note also drops its rows from `TaskStore` (`delete_for_note`). That
is a **deliberate exception** to "dismissed rows are retained as labelled
negatives" — everywhere else a decision is permanent corpus data, but an
explicit delete means gone, and keeping the rows would leave the corpus holding
labels for a note whose text no longer exists to explain them. Rows are removed
first, so a crash between the two strands nothing.

**Files on disk are never touched.** See "Files are referenced, never copied".

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

- **The block model needed no extension change, and no log out.** Attachments,
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

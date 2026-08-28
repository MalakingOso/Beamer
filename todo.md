# Beamer TODO

## Packaging / Release

- [ ] **Bundle GNOME extension in release artifacts** — No release pipeline exists yet (no CI, no build scripts, no `cargo-deb` config; `installer/` only has `.bmp` assets for a never-built Windows NSIS installer). When a release pipeline is created, copy `extension/beamer-focus@beamer.app/` into the artifact so `locate_source_dir()` can find it. Preferred placement (FHS fallback 2): `<artifact_root>/share/beamer/extension/beamer-focus@beamer.app/`. Acceptable fallback (portable fallback 3): `<artifact_root>/extension/beamer-focus@beamer.app/`. Dev/cargo-run fallback (fallback 4) already works because the directory sits at the repo root.

## Features
- [ ] ElevenLabs usage dashboard — API supports `GET /v1/user/subscription` (character_count/character_limit) and `GET /v1/usage/character-stats` (historical data with aggregation). Mistral has no usage API.
- [ ] Overlay window — the `Overlay` component exists but isn't wired to a separate transparent window for showing live transcription text on screen

## Sticky Notes — omissions

- [x] **The Windows target does not compile, in more places than were written
      down.** Done: the platform-neutral matching layer (`MAX_BINDINGS`,
      `BindingConfig`, `BindingState`, `Modifiers`, `build_bindings`,
      `matching_binding`) was hoisted from `linux_hotkey.rs` into
      `hotkey/mod.rs`, and `ll_hook.rs` was rewritten around it:
      `start_ll_hook`/`update_configs` now match `app.rs`'s calls, both
      `RecordStart` sites carry a `CaptureMode`, and the Windows hook can
      represent a second (note) binding. `cargo xwin check --target
      x86_64-pc-windows-msvc` is clean; runtime behaviour is still unverified
      on real Windows hardware.
      ⚠️ A plain `cargo check --target x86_64-pc-windows-msvc` does not work on
      this Linux host and never will without `xwin`: `ring` enters the
      dependency tree through `self_update` 0.42 (which force-enables
      reqwest's `rustls-tls` feature), and `ring`'s build script wants MSVC's
      `lib.exe`, which a Linux toolchain does not have. `cargo xwin check`
      supplies the MSVC libs itself, which is why that is the working gate:
      `XWIN_ACCEPT_LICENSE=1 cargo xwin check --target x86_64-pc-windows-msvc`.
      A **native** build from a Windows machine has `lib.exe` on its own and
      was never affected by this; cross-compiling from Linux is the only thing
      that was ever blocked.
- [x] **`HotkeyConfig` cannot express Super as a modifier.** Done: `parse()`
      now returns `None` when a Super/Win/Cmd/Meta token appears together
      with another key, instead of silently dropping Super and keying off the
      bare trigger. `Super` alone as the trigger still works (`Ctrl+Super`,
      `Ctrl+Alt+Super`), pinned by `super_alone_still_parses_as_the_trigger`.
- [x] `src/notes/task_store.rs` is **474 lines against the 500 limit**. Done:
      tests moved to `src/notes/task_store/tests.rs`, reached by `#[path]`.
      `prompts.rs`, `extract.rs` and `tasks_page.rs` were split the same way
      for the same reason.
- [ ] **Suggestion count badge on the notes board.** The spec (§9) calls for a
      note with pending suggestions to show a count on its card in the board.
      The Phase 2/3 plan did not ask for it and it was not built. Small: the
      board would need the `TaskStore` signal as a prop and
      `suggested_for(&id).len()`.
- [x] Note windows are placed but their **size** is never captured. Done: a
      `WindowEvent::Resized` arm in `StickyNote` writes `Note::size`, and
      `.sticky-grip` gives an undecorated note something to resize by. No
      extension change and therefore no log out — `xdg_toplevel.resize` is
      client-initiated, unlike positioning. (Position is still forgotten *by
      design* — see `agent_docs/sticky_notes.md`.)
- [ ] `GetWorkArea` extension method. Placement insets a fixed 40px for the
      GNOME panel; a real work area would account for docks and other struts.
      Speculative, so deferred — and it costs only the log out that any other
      extension change costs anyway. `GetWindowFrame` already ships unused and
      is the verification path for it.
- [ ] **Phase 3's extraction prompt is untuned.** It was spot-checked against
      the live model (five probes, all correct, including aspirations phrased
      like commitments) but never measured against a corpus, because none
      existed. `cargo run --bin task_eval` grades it against your own
      accept/dismiss decisions; run it once a few dozen notes have accumulated.
      If precision is poor, the levers are the prompt and the model ladder —
      **not** `min_confidence`, which measured 0.90-0.98 across every probe and
      filters approximately nothing.
- [ ] `assets/styles.css:752` references `var(--bg-elevated)`, which is not
      defined in the token block. Pre-existing since `c709faa`, unrelated to
      notes; an undefined custom property fails silently.

## Live Sync, open questions

- [x] **Attachment bytes now have a transport, decided and documented, not yet
      run.** The owner's decision: Syncthing carries `<config_dir>/sync/attachments`,
      independently of `automerge::sync`, which keeps carrying `notes.automerge`
      and nothing else. `agent_docs/sync.md`'s "Attachments: Syncthing carries
      the bytes" section has the full design (what Syncthing must and must not
      point at, why content-addressed immutable blobs need none of the
      conflict-copy machinery a live document would, setup steps for both
      machines) and says plainly that it is installed on neither machine yet.
      Nothing here has been run end to end. What is left: install it on both
      machines per those steps, verify a dropped attachment actually appears
      on the other side, and confirm Ignore Delete behaves as documented.
- [x] **The local half of cross-machine attachment deletion is decided and
      implemented; the cross-machine half still is not.** `release_attachment_bytes`
      (`src/notes/edit.rs`) now checks `NoteStore.sync_enabled`
      (`config.sync.url` non-empty, set from `sync_client::use_sync_client`)
      after its refcount check: with sync configured, a hash nothing local
      references any more is left on disk rather than deleted, because the
      local refcount has no visibility into whether a note open on another
      machine still needs those bytes. This trades disk for safety on
      purpose: an orphaned file costs space and can be cleaned up later, while
      a referenced image deleted on every machine at once cannot be recovered
      by anything. Tests: `with_sync_configured_deleting_the_last_reference_keeps_the_file`,
      `with_sync_configured_removing_the_last_reference_to_an_attachment_keeps_the_file`,
      and `without_sync_configured_the_refcounted_delete_still_removes_the_file`
      pin sync-on and sync-off behaviour side by side, in `src/notes/edit/tests.rs`.
      What remains, genuinely open:
      1. **Orphan collection has no story yet, and that is fine for now.**
         Bytes kept alive by the policy above accumulate with no local
         reference and nothing sweeps them. Needs an actual cross-machine
         notion of "referenced nowhere" (a tombstone/grace-period, a
         "still wanted elsewhere" check against sync state, or something
         else) before it is worth building; there is nothing to design
         against yet without real usage data on how much this actually
         accumulates.
      2. **The remove button gives no different warning on a machine that
         never held the original.** On the machine where an attachment was
         dropped, deleting it removes only Beamer's copy, the user's
         original file is never touched, by design. On a machine where the
         note and its attachment arrived entirely through sync, there never
         was a "user's original" to fall back on; today's UI gives no
         confirmation dialog that treats that case differently from the
         harmless one. The deletion-policy item above stops that keystroke
         from destroying the bytes immediately, but a clearer warning in the
         UI is still worth doing.
- [ ] **The missing-file card does not notice a file arriving.** Checked
      while wiring the Syncthing attachment transport above: `AttachmentBlock`
      in `src/ui/sticky_blocks.rs` computes `missing` once, from a plain
      `resolved.exists()` check, when it renders. Nothing polls afterward, and
      because it takes no props that a stray re-render would happen to change,
      Dioxus's prop-equality check skips re-running it on an unrelated parent
      render, so once a note shows the missing-file card it keeps showing it
      until something changes that note's own content (a real edit) or the
      window is closed and reopened, even though the attachment's bytes may
      have landed on disk seconds later. That is exactly the shape this
      feature produces on purpose (see `agent_docs/sync.md`, "a note can
      arrive before its image does"), so this matters: the card should heal
      itself, most naturally with a short `use_future` inside
      `AttachmentBlock` that polls `exists()` while `missing` is true and
      stops once it flips. Not built here: `sticky_blocks.rs` was locked to a
      concurrent edit for the whole of this task.
- [ ] **`task_eval` cannot read a pre-Task-8 `notes.json` that carries an
      attachment, until Beamer has run once and flushed.** `NoteStore::load`
      upgrades a legacy path-shaped attachment (`{"kind":"image","path":
      "..."}`) to the current shape (`{"kind":"image","filename":"...",
      "location":{...}}`) in memory on every load, but that upgrade only
      reaches disk on the next `flush_if_dirty()`. `src/bin/task_eval.rs`
      `#[path]`-includes `src/notes/model.rs` directly and parses `notes.json`
      with `Attachment`'s own strict `Deserialize`, no shape-upgrade pass,
      because that logic lives in `mod.rs`, which is not includable (it
      reaches for `crate::config::Config`). So: on an install that had
      attachments before Task 8 and has not opened Beamer since upgrading,
      `cargo run --bin task_eval` fails outright on that file until Beamer
      itself has run once. Not a bug to fix in `task_eval.rs`; teaching it
      the legacy shape too would duplicate migration logic in a second place.
      Just a real ordering constraint worth having written down before
      someone hits it and assumes `task_eval` is broken.

## Sticky Notes — surveyed and deliberately deferred

Considered while planning inline attachments and left out on purpose, so nobody
re-derives the list from scratch. None of these is blocked; each is simply not
worth its complexity yet.

- [ ] **Dropping *between* two text runs.** A drop appends at the end of the
      body today. Per-run drop zones are the obvious next step and are cheap —
      `blocks::insert_token` already takes a position argument in spirit; it
      just needs a run index instead of a bool.
- [ ] **Backspace at the start of a run deletes the block above it.** Not
      promised, and deliberately not attempted: Dioxus `KeyboardData` carries no
      caret position, so knowing `selectionStart == 0` needs a `document::eval`
      roundtrip per keydown. The hover `⤫` on each attachment block is the
      contract. Try this only if the roundtrip turns out not to be janky.
- [ ] **Pasted image bytes.** Out of scope while attachments are
      reference-by-path: a screenshot on the clipboard is not a file anywhere.
      Would need Beamer to own a media directory, which is the decision that was
      explicitly not taken. The note detects the case and points at the
      paperclip rather than silently dropping the paste.
- [ ] **Thumbnails on the notes board.** The board would need its own asset
      handler (handlers are per-window) for a strip nobody reads at card size.
      Cards show `📎 n` instead.
- [ ] Tags, collapsible notes, in-note checklists, a per-note workspace, and
      audio playback of the original dictation. All surveyed, none scoped.
- [ ] **Live two-way calendar entries.** `.ics` export is one-way: Beamer cannot
      edit or remove what the calendar imported. The upgrade path is Evolution
      Data Server over the `zbus` dependency Beamer already has — not planned.
- [ ] **Grade dates in `task_eval`.** `extract_system(today)` and the four
      validation gates are unmeasured, exactly like the extraction prompt they
      extend. `task_eval` already passes each note its own capture date, so the
      harness is ready; it needs the accept/dismiss corpus to grow first.

## Low Priority
- [ ] `overlay_enabled` config field is never read — wire it to conditionally show/hide the glow overlay
- [ ] `debug_logging` toggle saves to config but has no runtime effect — consider wiring it to control injection trace logging
- [ ] Auto-start toggle in settings UI — `auto_start` config field exists but there's no settings control for it
- [ ] `set_auto_start(false)` code path is unreachable — no UI to disable auto-start once enabled

## Done
- [x] Quick fix: skip SendInput for Warp (process-name detection, route to clipboard)
- [x] Fix vocabulary persistence — terms now save immediately on add/remove instead of only on "Save Changes"
- [x] Fix hotkey picker — redesigned with modifier checkboxes (Ctrl/Alt/Shift/Win) + key capture button. No more freezing.
- [x] Fix screen edge glow — Glow component now renders inside main window when recording, with pulsing animation
- [x] Rename "Hold-to-talk" to "Push to Talk" everywhere in the UI
- [x] Add 400ms release delay after hold-to-talk — continues capturing audio briefly so last word isn't clipped
- [x] Remove acrylic backdrop — switched to solid backgrounds, removed DWM backdrop code and Win32_Graphics_Dwm feature
- [x] History copy button — now uses Phosphor Copy/Check icons with green feedback flash on copy
- [x] Fix settings window visual glitches — added overflow-x:hidden, min-width:0 on flex containers
- [x] API usage research — ElevenLabs has usage endpoints, Mistral does not
- [x] Remove dead HotkeyHandler code — stripped hotkey/mod.rs to just the HotkeyEvent enum, removed #![allow(dead_code)]
- [x] Deduplicate load_api_key/save_api_key — moved to config/mod.rs, removed copies from orchestrator.rs and settings/mod.rs
- [x] Clean up dead code — removed OverlayApp, GlowApp, AdvancedConfig, Vocabulary::import_from_file, unused CSS classes

# Beamer TODO

## Packaging / Release

- [ ] **Bundle GNOME extension in release artifacts** — A release pipeline exists now: `.github/workflows/windows.yml` builds and tests on Windows AND Linux. The Windows job uploads a portable `beamer.exe` plus an NSIS installer (`dx bundle --package-types nsis`); the Linux job uploads `sync_server` plus a `.deb` (`dx bundle --package-types deb`, which carries `sync_server` into the package via `[bundle.deb.files]` in `Dioxus.toml`). What is still missing: the GNOME extension is not bundled — copy `extension/beamer-focus@beamer.app/` into the artifact so `locate_source_dir()` can find it. Preferred placement (FHS fallback 2): `<artifact_root>/share/beamer/extension/beamer-focus@beamer.app/`. Acceptable fallback (portable fallback 3): `<artifact_root>/extension/beamer-focus@beamer.app/`. Dev/cargo-run fallback (fallback 4) already works because the directory sits at the repo root.

## Features
- [ ] ElevenLabs usage dashboard — API supports `GET /v1/user/subscription` (character_count/character_limit) and `GET /v1/usage/character-stats` (historical data with aggregation). Mistral has no usage API.
- [ ] Overlay window — the `Overlay` component exists but isn't wired to a separate transparent window for showing live transcription text on screen

## Sticky Notes — omissions

- [ ] **Suggestion count badge on the notes board.** The spec (§9) calls for a
      note with pending suggestions to show a count on its card in the board.
      The Phase 2/3 plan did not ask for it and it was not built. Small: the
      board would need the `TaskStore` signal as a prop and
      `suggested_for(&id).len()`.
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

## Live Sync, open questions

- [ ] **Install Syncthing for attachment bytes on both machines and verify end
      to end.** Design and setup steps are done — `agent_docs/sync.md`,
      "Attachments: Syncthing carries the bytes" — but it is installed on
      neither machine yet. Verify a dropped attachment appears on the other
      side and that Ignore Delete behaves as documented.
- [ ] **Cross-machine orphaned attachment bytes have no collection story.**
      The local half of deletion is implemented (`release_attachment_bytes` in
      `src/notes/edit.rs` keeps a hash on disk when sync is enabled and the
      local refcount can't see whether another machine still needs it), but
      nothing sweeps those kept-alive bytes later. Needs a real cross-machine
      "referenced nowhere" notion (tombstone/grace-period or a check against
      sync state) — not worth designing until there's usage data on how much
      this actually accumulates.
- [ ] **No distinct delete warning for a machine that never held the
      original.** Deleting an attachment always removes only Beamer's own
      copy, never the user's source file — but on a machine where a note
      arrived entirely through sync, there never was a source file, and the
      confirm dialog doesn't say so.
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
- [ ] **Meeting capture / system audio / diarization.** Explicitly out of
      scope — surveyed, not scoped, not planned.

## Pending manual actions

- [ ] **Log out to deploy GNOME extension v6.** Bumped 2026-08-24 to deploy the
      pill-waveform change from 7f83eeb, which had sat undeployed since it was
      committed. Settings → the injection card offers the update; click it,
      then log out. Confirm with the `GetVersion` check in
      `agent_docs/sticky_notes.md` — expect `(uint32 6,)`.

## Low Priority
- [ ] `debug_logging` toggle saves to config but has no runtime effect — consider wiring it to control injection trace logging
- [ ] Auto-start toggle in settings UI — `auto_start` config field exists but there's no settings control for it
- [ ] `set_auto_start(false)` code path is unreachable — no UI to disable auto-start once enabled

## Done

Resolved and reflected in `git log` / commit messages — kept out of here.

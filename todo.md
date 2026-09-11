# Beamer TODO

## Packaging / Release

- [ ] **Bundle GNOME extension in release artifacts** — A release pipeline exists now: `.github/workflows/windows.yml` builds and tests on Windows AND Linux. The Windows job uploads a portable `beamer.exe` plus an NSIS installer (`dx bundle --package-types nsis`); the Linux job uploads `sync_server` plus a `.deb` (`dx bundle --package-types deb`, which carries `sync_server` into the package via `[bundle.deb.files]` in `Dioxus.toml`). What is still missing: the GNOME extension is not bundled — copy `extension/beamer-focus@beamer.app/` into the artifact so `locate_source_dir()` can find it. Preferred placement (FHS fallback 2): `<artifact_root>/share/beamer/extension/beamer-focus@beamer.app/`. Acceptable fallback (portable fallback 3): `<artifact_root>/extension/beamer-focus@beamer.app/`. Dev/cargo-run fallback (fallback 4) already works because the directory sits at the repo root.

## Features
- [ ] ElevenLabs usage dashboard — API supports `GET /v1/user/subscription` (character_count/character_limit) and `GET /v1/usage/character-stats` (historical data with aggregation). Mistral has no usage API.
- [ ] Overlay window — the `Overlay` component exists but isn't wired to a separate transparent window for showing live transcription text on screen

## Local inference — S1-mini cleanup, parked

- [ ] **Bring transcript cleanup back.** `[llm.cleanup] enabled` now defaults
      to `false` and has its own toggle on the Local AI settings card
      (2026-09-06, see `docs/decisions.md`). Nothing was deleted — `llm/
      cleanup.rs`, `prompts::CLEANUP_SYSTEM`, `pipeline::run_cleanup`, the
      per-stage `base_url` override and the whole `clean_state` lifecycle are
      all intact and tested. What it needs to work again is a server that
      actually serves the model: either an `[s1-mini-q4_k_m]` preset in
      `deploy/llama-models-bearcave.ini` plus the 462 MiB GGUF fetched into
      `%LOCALAPPDATA%\Beamer\...` the way `model_setup` fetches K2-Horizon,
      or a reachable GPU host. Do **not** substitute a different quant — the
      94.8% token-accuracy figure in `agent_docs/local_inference.md` was
      measured on `q4_k_m` specifically. Note the licence obligation
      (`llm::MODEL_CREDIT`) comes back with it.
- [ ] **Cleanup may want to be a dictation feature, not a notes feature.** The
      framing that prompted turning it off was "unrelated to the stickies":
      cleaning a transcript is arguably something dictation should do before
      *injection*, not something a note pass does afterward. Today it only ever
      runs over `notes`. Worth deciding before it is switched back on, because
      it changes where the toggle belongs and whether `clean_state` is still
      the right home for the result.
- [ ] **Re-enabling cleanup resurrects nothing.** While cleanup is off,
      `App()` marks every non-`Done` note `Skipped`, including dictated notes
      that were only ever `Pending` because the server happened to be down.
      Turning cleanup back on does not undo that: `sweep_requests` retries
      only `Failed`, so those notes sit at a quiet `Check` footer and need a
      per-note press to ever run. Deliberate for now — skipping `Pending` is
      exactly what stops the footer offering a "Clean up" affordance for a
      pass that cannot run — but if cleanup comes back as a routine feature,
      the toggle-on path probably wants to reset `Skipped` to `Pending` for
      `Dictated` notes. Raised in review by muse.
- [ ] **`failure_message` reports an unreachable host as a reachable one.**
      `src/llm/client.rs` checks `timed_out` before `connect_failed`, but
      reqwest sets `is_timeout()` for a *connect* timeout too — so a tailnet
      host that never accepted a TCP connection renders as "No response — the
      server is reachable but did not answer", which is the opposite of what
      happened and directly misled a real debugging session. Swap the two
      branches; the existing test `failures_are_described_in_the_users_terms`
      pins the current order and needs updating with it.
- [ ] **`succeeded_from` conflates two servers' reachability.** With per-stage
      `base_url`, one request can touch two hosts, but
      `pipeline/sweep.rs::succeeded_from` folds them into a single
      `succeeded` bool: any errored stage suppresses the backlog sweep, so a
      successful *local* extraction cannot trigger a sweep while a *remote*
      cleanup is down. Latent while cleanup is off (one host, one stage);
      fix it before re-enabling a split deployment. Probably wants the sweep
      trigger keyed by base_url rather than by request.

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

- [ ] **Verify the desktop hotkey grab on real hardware.** None of this is
      provable from tests. With `journalctl --user -f -o cat /usr/bin/gnome-shell`
      open: (1) press and release the chord and check for
      `beamer: hotkey 0 released (mutter)`; if it never appears, hold mode
      over the grab is broken. (2) `RUST_LOG=beamer=debug dx serve`: one
      `RecordStart` per press, one `RecordStop` per release, and Ctrl+Space in
      a text field no longer types a space. (3) Hold the chord, release Ctrl
      *first*: expect `released (modifiers)` and exactly one `RecordStop`.
      (4) Over RDP, both the plain hold and the Ctrl-first release (the
      modifier poll assumes `global.get_pointer()` sees gnome-remote-desktop's
      virtual keyboard; unverified). (5) Lock and unlock, press again. (6)
      Change the hotkey in Settings: old chord dead, new one live. (7) Pick a
      chord GNOME already binds: the grab log shows `false` for that slot and
      evdev still fires it. (8) Quit Beamer: Ctrl+Space types a space again.

## Low Priority
- [ ] `debug_logging` toggle saves to config but has no runtime effect — consider wiring it to control injection trace logging
- [ ] Auto-start toggle in settings UI — `auto_start` config field exists but there's no settings control for it
- [ ] `set_auto_start(false)` code path is unreachable — no UI to disable auto-start once enabled

## Done

Resolved and reflected in `git log` / commit messages — kept out of here.

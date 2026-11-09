# Beamer Roadmap

## Known issues

- An existing `config.toml` that can't be *read* (permissions, a lock) loads
  as defaults in memory, and the next Settings edit saves those defaults over
  the real file. Corrupt TOML is already set aside safely; this is only the
  I/O-error path (`Config::load().unwrap_or…` in `main.rs` and `ui/app.rs`).

## Verify on Linux

The 2026-09-30 stability pass was built and tested on Windows only. These
Linux-only edits have never been compiled:

- `hotkey/linux_hotkey.rs`: `release_held_keys` when a keyboard's listener
  exits (unplug mid-hold); poison-tolerant binding locks
- `injection/ydotool.rs`: `ydotool type` bounded by `run_with_timeout`
- `notes/sync_doc/mod.rs`: `is_transient_rename_error` is `false` off Windows
- `hotkey/linux_hotkey.rs`, `hotkey/gnome_grab.rs`: the third (Done) binding
  slot — `MAX_BINDINGS` is 3 and `update_configs`/`start_ll_hook` take a
  `done` chord. `SetHotkeys` already takes any number of accelerators, so no
  extension bump is needed, but the Linux path is uncompiled
- `ui/app_day_end.rs`: the Linux branch of `show_card` (a plain
  `show_notification` with the summary text) and the `cfg` gating around the
  card window have never been compiled on Linux

## Day-end card: come back to this

The gold-frame card (2026-11-08) is the first cut: a card at a set time that
lists the day's Done lines. The design exploration around it was half-baked
and is parked for a second pass once the card has been lived with. The boards
(colour A1-A6, format B1-B6, concept C1-C6, plus the gold frame built out) are
in the "End-of-day Done ideas" artifact, claude.ai/artifact/XPqpE5y82jtUWA3NaJfLYJ.
Not decided yet, and not built:

- Colour, format and concept: the first cut is A1 (gold on white) + B1 (full
  list) + a plain card. Dark glass (A2) + pill-unfolds (C1) was the other
  front-runner.
- "Shown today" is not persisted: starting after the set time counts as
  already shown, so a restart at 17:35 does not repeat it. Persist it
  (machine-local, like `machine.json`) if that proves too blunt.
- The card appears on the primary monitor's work area, not the foreground
  monitor the way the recording pill does (`app_pill::reposition_to_foreground_monitor`).
- The reveal skips the "body opens downward" step: slide in, then lines land.
- It is a custom window, so it ignores Focus Assist and can sit over a
  full-screen app. A real toast would respect both but cannot draw the card.
- No sound. A soft chime (`sounds.rs`) is a design choice, not a constraint.
- A day with nothing logged at the set time is skipped for the whole day, so
  a line logged at 17:40 gets no card (use "Show it now"). Chosen so the card
  never pops up mid-dictation; revisit if people miss it.
- A late wake shows the card only within 60 minutes of the set time; the
  window is fixed, not a setting.
- Linux gets a plain `show_notification` with the summary text. A real card
  needs Wayland placement help and a `done` state in the GNOME pill (extension
  version bump and a log out).
- The card is non-focusable, so clicks reaching it on real Windows hardware
  (close, Copy list, Open Done) need checking by hand.

## Low priority

- Notes/tasks `save()` and the 500 ms flush run on the UI thread and rewrite
  whole files. Fine at today's size; revisit if the corpus gets large
  (`notes/mod.rs`, `notes/flush.rs`, `ui/app_setup.rs`).

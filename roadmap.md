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

## Low priority

- Notes/tasks `save()` and the 500 ms flush run on the UI thread and rewrite
  whole files. Fine at today's size; revisit if the corpus gets large
  (`notes/mod.rs`, `notes/flush.rs`, `ui/app_setup.rs`).

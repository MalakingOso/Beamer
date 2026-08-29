# Design history

One-paragraph summaries of the design specs and plans that used to live under
`docs/plans/` and `docs/superpowers/` — all pre-implementation planning
documents for features that have since shipped. Full technical detail for how
each feature actually works lives in `agent_docs/`, which stays current as the
code changes; these entries are just the "why," kept so the reasoning behind a
decision doesn't have to be re-derived from git log. The original spec/plan
text is still recoverable from git history if the full detail is ever needed.

## Deploy Purple design language (2026-03-01)

Migrated the UI from an earlier "Developer Precision" look to Deploy Purple: a
`#4B0082` purple accent, 2px borders, hard-offset shadows, DM Mono + Recursive
fonts. CSS-only redesign plus a couple of small Rust changes for window
transparency. See `agent_docs/design_system.md`.

## Status log feed (2026-03-01)

The orchestrator's lifecycle events (connection, session start, errors) only
reached `tracing`/the terminal; the UI had no visibility into what the
WebSocket connection was doing. Added a `TranscriptKind` split (was a flat
`is_final` bool) and an in-memory log ring buffer surfaced on the Debug card.

## Taskbar icon + hotkey picker (2026-03-01)

Two small features bundled together: setting the Beamer icon on the Windows
taskbar (one line on `WindowBuilder`), and a "listening" mode on the
Recording card that captures a key combo live instead of requiring hand-typed
chord strings.

## GNOME focus-aware paste shortcut (2026-04-22)

Clipboard injection needs Ctrl+V for most apps and Ctrl+Shift+V for terminals,
and guessing wrong silently no-ops — `ydotool` reports success either way
since the keystroke itself was synthesized correctly. Bundled a minimal GNOME
Shell extension exposing the focused window's app id over D-Bus
(`app.beamer.FocusProvider.GetFocusedAppId`), classified against an
exact-match terminal list. This was the extension's first version; it has
since grown typing and window-placement methods too. See
`agent_docs/text_injection.md` and `agent_docs/sticky_notes.md`.

## Linux injection v2 + shell-native pill (2026-07-18)

The original Linux chain (`ydotool → clipboard`) failed in real contexts:
`ydotool type` is ASCII-only and assumes US QWERTY, so any non-transliterable
character aborted the whole backend. Extended the GNOME extension to type
directly via `Clutter.VirtualInputDevice` (the `gnome` backend), added `wtype`
for wlroots compositors ahead of `ydotool`, and moved the recording pill from
a Dioxus overlay window to a Shell-native St widget on GNOME. See
`agent_docs/text_injection.md`.

## Pill overlay polish (2026-07-19)

Follow-up to the shell-native pill: made it appear on the focused window's
monitor rather than always the primary one, and restyled it to Deploy Purple
(solid near-white capsule, no acrylic — Shell overlays can't blend). The
Windows/macOS Dioxus pill was untouched by this pass; it got its own Deploy
Purple pass later (see recent commits).

## Sticky notes, all three phases (2026-08-20 onward)

A second global hotkey dictates into a sticky note instead of injecting into
the focused field, with two on-device model passes (cleanup, then task
extraction) running against a local llama.cpp server. This was the largest
feature built this way — capture and placement (phase 1), an S1-mini cleanup
pass (phase 2), and Gemma-based task-suggestion chips (phase 3). Placement in
particular went through real design churn (position persistence was
attempted and deliberately dropped once it turned out Wayland gives clients
no way to read their own window geometry back). Full detail, including the
gotchas that cost real time to find, is in `agent_docs/sticky_notes.md` and
`agent_docs/local_inference.md` — those are the documents to read before
touching this code, not the original spec.

## B60 llama.cpp benchmark (2026-08-21)

The sticky-notes spec's latency numbers were all estimates. This measured the
real ones on the Arc Pro B60 and, in the process, is what led to choosing
SYCL over Vulkan as the inference backend (2.35x faster at prompt
processing). All current numbers live in `agent_docs/local_inference.md`'s
"Measured, so stop estimating" section — this entry exists only to record
that the SYCL-vs-Vulkan choice was a measured decision, not a default.

## Windows target compiles + remote model story (2026-08-27)

Beamer is Windows-first by design but months of work happened on Linux, and
the Windows target had silently stopped compiling (the note-hotkey feature
only landed on the Linux hotkey hook). Fixed the Windows hook to share the
same binding-matching layer as Linux, and settled the remote-models question:
the laptop has no GPU capable of running the models, so Windows Beamer talks
to the Linux desktop's llama-server over Tailscale rather than shipping a
second inference stack. Since verified running on real Windows hardware
(x86_64 and Windows ARM) — see `agent_docs/running_on_bearcave.md`.

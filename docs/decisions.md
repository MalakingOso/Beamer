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

## Extraction moves off callisto onto bearcave itself (2026-09-04)

The "no GPU capable of running the models" premise above turned out to hold
for cleanup but not extraction. A sandbox investigation
(`k2-horizon-test/HANDOFF.md`) found that K2-Horizon-0.9B at Q8_0 (1.15 GB)
runs acceptably CPU-only, on the same Windows-on-ARM laptop that has no
llama.cpp GPU backend at all — a 0.9B model doesn't need one. Split
`LlmConfig::base_url` into per-stage overrides
(`cleanup_base_url()`/`extract_base_url()`, `src/llm/mod.rs`) so extraction
could point at a local server while cleanup keeps using callisto over
Tailscale, and made K2-Horizon-0.9B-Q8_0 the new default extraction model.
This is a real architecture change, not a config tweak: K2-Horizon requires a
different llama.cpp fork than either existing server builds from
(`MBZUAI-IFM/llama.cpp`, branch `model/K2Horizon` — upstream cannot load the
architecture at all), and bearcave now runs its own standalone `llama-server`
for the first time, launched by a Windows Scheduled Task rather than a
systemd unit. See `agent_docs/local_inference.md` and
`agent_docs/running_on_bearcave.md` for the full detail, including two open
extraction-quality gaps this did not fix (an occasional third-party-commitment
misattribution, and relative-date math past "tomorrow") and the model's
unresolved licence terms.

## Cleanup becomes opt-in, and everything runs on device (2026-09-06)

The previous entry left cleanup on callisto and moved only extraction local.
That split was invisible until callisto stopped answering, at which point
every dictated note came back with a red "Cleanup failed" footer while the
note itself looked perfectly clean — because ElevenLabs already returns
punctuated, capitalized text, so `raw == body` on a note s1-mini never
touched, and extraction (local, working) still produced its tasks. The footer
was telling the truth about a pass the user had no way to turn off on its own:
`[llm.cleanup] enabled` existed in the config but the settings card exposed
only one combined switch for both stages. Worse, the failures could not clear
themselves — `succeeded_from` requires *no* errored stage, so a cleanup error
against an unreachable host suppressed the backlog sweep that a successful
local extraction had otherwise earned.

So: `CleanupConfig::enabled` now defaults to `false` and gets its own toggle on
the Local AI card, independent of the master `[llm] enabled`. This is the
honest default rather than a workaround — `model_setup` installs K2-Horizon and
nothing else, so on a fresh install cleanup was *always* going to ask a server
for a model it had never been told to serve. Extraction is the pass that earns
its place on a sticky. To keep the change from leaving a graveyard behind,
`App()` runs `NoteStore::skip_cleanup_on_every_note` while cleanup is off,
downgrading leftover `Failed`/`Pending` cleanup stages to `Skipped`; the
pipeline only visits notes it is asked about, so without that the old failures
would sit on disk and re-flash on every window open. Per-stage `base_url`
support and the whole cleanup path are kept intact — bringing S1-mini back
needs a local s1-mini preset (or a reachable GPU host), not new code. See
`todo.md`.

## Desktop-level hotkey on GNOME, so RDP chords work (2026-09-11)

Remoted in over gnome-remote-desktop, the dictation chord was dead: RDP input
is injected inside Mutter through virtual devices and never reaches
`/dev/input`, which is all the evdev listener reads. The chord is now also
grabbed inside the Shell extension (`grab_accelerator`, v7) and re-emitted over
D-Bus. XGrabKey was never an option (GNOME 50 has no X11 session), and the
GlobalShortcuts portal would have cost an approval dialog, app-id registration
for a non-sandboxed `dx`-run binary, and bindings the user can remap out from
under the settings card. The extension is already the first-class GNOME
Wayland path and Mutter exports both `accelerator-activated` and
`accelerator-deactivated`, which is what hold-to-talk needs. A grabbed chord is
consumed, so Ctrl+Space no longer types a space into the focused app; Windows
already behaves that way (`ll_hook.rs` swallows it). Evdev stays as the
fallback, per binding: a Super trigger (collides with the overlay key), a
chord GNOME already owns, an old helper, or a locked screen all leave that
binding on evdev exactly as before. One behaviour differs between the paths:
through the grab a hold ends when *any* chord key goes up (Mutter can't match a
release once Ctrl is gone, so the extension watches the modifiers), where
evdev ends it only on the trigger key. See `agent_docs/text_injection.md`.

## Cleanup pass deleted outright (2026-09-12)

The opt-in cleanup pass is gone. ElevenLabs already returns punctuated,
capitalized text, so a transcript normalizer had nothing to fix, and every
fresh install paid for the attempt with a red footer label for a model its
server was never told to serve. Keeping it opt-in only hid that cost; the
pass earned no place on a sticky either way.

Deletion spans the whole stack: `src/llm/cleanup.rs` and `CleanupConfig`, the
`[llm.cleanup]` config section, the Local AI card toggle, the s1-mini section
of `deploy/llama-models.ini`, and `licenses/S1-mini-LICENSE.txt`. Notes
written while cleanup existed load fine without it (the stale `clean_state`
key is ignored on load and dropped on the next flush, in JSON and in the
sync document alike).

The shared client code stays. `chat.rs`, its error classification and the
footer failure message all serve extraction too, so the `failure_message`
roadmap bullet (unreachable hosts misreported as reachable) survives on
that code. It was never cleanup-specific.

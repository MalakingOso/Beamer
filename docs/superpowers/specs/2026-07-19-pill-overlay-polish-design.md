# Pill Overlay Polish — Active Monitor, Deploy Purple, No Timer

Date: 2026-07-19. Linux-only: all changes in `extension/beamer-focus@beamer.app/`.
The Windows/macOS Dioxus pill (`src/ui/pill.rs`) is intentionally untouched.

## Requirements

1. **Active monitor** — on multi-monitor setups the pill must appear on the
   monitor of the currently focused window (where dictated text lands), not
   the primary monitor. Fallback when nothing has focus: the pointer's
   monitor (`Main.layoutManager.currentMonitor`), then primary.
2. **Deploy Purple styling** — the pill must match the app design system
   instead of the dark VibeTyper glass capsule:
   - solid near-white surface `#fbfbfd` (solid equivalent of the card
     surface token; shell overlays have no acrylic)
   - `2px solid rgba(75, 0, 130, 0.25)` border (the `--border-strong` token)
   - `border-radius: 8px` (design-system max — no longer a capsule)
   - hard-offset shadow `2px 4px 0 0 rgba(75, 0, 130, 0.15)`, zero blur
   - text: DM Mono/monospace, `#0f152a` (`--fg`)
   - waveform bar gradient restricted to the accent ramp
     `#4B0082 → #5C1A9E` (`--accent` → `--accent-hover`)
3. **No timer** — the elapsed-time counter is removed entirely. Recording
   shows waveform bars only; processing keeps the "Transcribing…" label.

## Changes

- `indicator.js` — `_reposition()` resolves
  `global.display.focus_window?.get_monitor()` →
  `Main.layoutManager.monitors[i]` with fallbacks; timer label/source and
  `_stopTimer()` deleted; `COLOR_TO` becomes `#5C1A9E`.
- `stylesheet.css` — rewritten to the tokens above; `.beamer-pill-timer`
  rule deleted; `min-width` dropped so the pill hugs its content.
- `metadata.json` + `extension.js` — version/`HELPER_VERSION` bumped to 3 so
  the app's install-status logic flags the running v2 as stale.
- `agent_docs/text_injection.md` — indicator description updated.

## Deployment / testing

Reinstall the extension and log out/in (Wayland loads extension code only at
login). Then dictate with focus on each monitor and confirm the pill follows;
visually confirm Deploy Purple styling and absence of the timer.

# Beamer TODO

## High Priority
- [ ] Fix hotkey picker — freezes waiting for input, doesn't work. Need to support modifier combos like Ctrl+Win
- [ ] Fix screen edge glow — the whole glow color effect doesn't work at all
- [ ] Rename "Hold-to-talk" to "Push to talk" everywhere in the UI
- [ ] Add a short delay (e.g. 300–500ms) after releasing hold-to-talk before stopping recording — last word often gets cut off when releasing the key too early

## Features
- [ ] Replace TOML-based vocabulary storage with SQLite database
- [ ] API usage/limits dashboard — check if Mistral or ElevenLabs APIs expose usage/quota endpoints
- [ ] Fix settings window visual glitches when expanding/spreading to the right

## UI Polish
- [ ] Remove acrylic backdrop
- [ ] History copy button — make it bigger, use Phosphor icon, add visual feedback (flash/toast) on copy
- [ ] Copy button should give a clear cue that the copy succeeded

## Injection Improvements
- [ ] Post-injection verification — after SendInput, check if text actually appeared in the focused element (e.g. re-read IValueProvider or ITextPattern value), fall through to clipboard if unchanged
- [ ] Try `PostMessage(WM_CHAR, ...)` as an alternative injection method — send character messages directly to the target window handle instead of using SendInput

## Done
- [x] Quick fix: skip SendInput for Warp (process-name detection, route to clipboard)

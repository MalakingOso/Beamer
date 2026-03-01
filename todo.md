# Injection Improvements

## Done
- [x] Quick fix: skip SendInput for Warp (process-name detection, route to clipboard)

## TODO
- [ ] Post-injection verification — after SendInput, check if text actually appeared in the focused element (e.g. re-read IValueProvider or ITextPattern value), fall through to clipboard if unchanged. This would catch *any* app that silently drops KEYEVENTF_UNICODE events without needing a hardcoded process list.
- [ ] Try `PostMessage(WM_CHAR, ...)` as an alternative injection method — send character messages directly to the target window handle instead of using SendInput. Some custom-input apps (Electron, GPU-rendered terminals) handle posted WM_CHAR messages better than synthetic SendInput events. Could be inserted between SendInput and Clipboard in the fallback chain.

# Text Injection — UIA Fallback Chain

## Overview

Text injection is the most critical subsystem. The goal: transcribed text appears exactly where the user's cursor is, regardless of which application is focused.

## Fallback Chain (in order)

### 1. UIA SetValue (preferred)
- Get focused element via `IUIAutomation::GetFocusedElement()`
- Query for `IValueProvider` pattern
- Call `SetValue(text)` — atomically sets text
- **Pros:** Clean, fast, respects undo history in some apps
- **Cons:** Not all controls support `IValueProvider`

### 2. UIA SendKeys
- Get focused element, verify it supports text input
- Use `ITextPattern` if available to get insertion point
- Fall back to `SendKeys` via UI Automation
- **Cons:** Slower, may trigger autocomplete/suggestions

### 3. Win32 SendInput
- Convert each character to `INPUT` structs with `KEYEVENTF_UNICODE`
- Handle Unicode surrogate pairs for emoji/CJK
- Call `SendInput()` with array of inputs
- **Pros:** Works in most apps, handles Unicode
- **Cons:** May be blocked by some security software, triggers key event handlers

### 4. Clipboard Paste (last resort)
- Save current clipboard contents
- Set clipboard to transcribed text
- Simulate `Ctrl+V` via SendInput
- Wait 500ms, restore original clipboard
- **Pros:** Universally works
- **Cons:** Destructive to clipboard, timing-sensitive

## COM Threading

All UIA calls MUST happen on a thread with COM initialized:
```rust
tokio::task::spawn_blocking(|| {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok();
    }
    // ... UIA calls here ...
    unsafe { CoUninitialize(); }
})
```

## Per-App Workarounds

| App | Issue | Workaround |
|-----|-------|------------|
| VS Code | Electron, UIA partially works | SendInput preferred |
| Chrome address bar | UIA SetValue works | Use UIA |
| Chrome textarea | IValueProvider not available | SendInput or clipboard |
| Discord | Electron, custom input | SendInput |
| Word | Rich text, UIA works well | UIA SetValue |
| Notepad | Standard Win32 edit | UIA SetValue works |

## Debug Logging

Log every injection attempt:
- Target app name (from window title / process name)
- Method attempted and result (success/fail)
- Fallback chain traversal
- Final method that succeeded

## Control Type Detection

Use `IUIAutomationElement::CurrentControlType` to identify the focused control:
- `UIA_EditControlTypeId` — standard text input
- `UIA_DocumentControlTypeId` — rich text (Word, etc.)
- `UIA_ComboBoxControlTypeId` — dropdown with text input
- `UIA_CustomControlTypeId` — custom controls (Electron apps)

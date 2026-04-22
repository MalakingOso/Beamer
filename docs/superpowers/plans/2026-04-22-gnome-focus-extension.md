# GNOME Focus-Aware Paste Shortcut Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bundle a minimal GNOME Shell extension with Beamer that exposes the focused window's app id over D-Bus, and use it to pick Ctrl+V or Ctrl+Shift+V per-app. Every failure mode falls back to the current Ctrl+Shift+V default.

**Architecture:** GNOME extension exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` inside the Shell process. Rust side calls it via `zbus` blocking client, classifies via an exact-match terminal list, picks shortcut, falls back on any failure. Install helper runs file copy + `gnome-extensions enable`; the Settings card surfaces status and an Install button. Spec: `docs/superpowers/specs/2026-04-22-gnome-focus-extension-design.md`.

**Tech Stack:** Rust (Dioxus 0.7 + tokio + zbus 5), GJS (GNOME 48+ extension class, ESM imports), D-Bus session bus.

---

## File Structure

Each file has one clear responsibility.

| Path | Responsibility | Status |
|---|---|---|
| `extension/beamer-focus@beamer.app/metadata.json` | Extension manifest (uuid, shell-version 48/49/50) | new |
| `extension/beamer-focus@beamer.app/extension.js` | ESM extension class. On enable() exports D-Bus object at `/app/beamer/FocusProvider`. On disable() unexports. Reads `global.display.focus_window` on each call. | new |
| `Cargo.toml` | Add `zbus = "5"` dep | edit |
| `src/injection/focus.rs` | Terminal classification (pure) + zbus client for `GetFocusedAppId` | new |
| `src/injection/mod.rs` | `pub mod focus;` wiring | edit |
| `src/injection/clipboard.rs` | `resolve_use_shift_v()` refactored into pure `choose_paste_shortcut(setting, focused)` + thin caller that injects real `focused_app_id()` | edit |
| `src/config/mod.rs` | `default_paste_shortcut()` returns `"auto"` | edit |
| `src/install/mod.rs` | Module root, exports `gnome_extension` | new |
| `src/install/gnome_extension.rs` | Status probe (parses `gnome-extensions list --details`), install (file copy + enable), uninstall (disable + rm -r). Source-path resolution: `$BEAMER_EXTENSION_DIR` env, then packaged, then portable. | new |
| `src/main.rs` | `pub mod install;` wiring | edit |
| `src/ui/settings/injection_card.rs` | Add "Auto (recommended)" option to Paste-shortcut Select; add "GNOME focus helper" subsection with status + Install/Remove buttons | edit |
| `agent_docs/text_injection.md` | Document the detection chain and install flow | edit |

Total: 7 new files, 6 edited.

---

## Testing note

The repo has no `#[cfg(test)]` modules today. This plan introduces unit tests only for the genuinely testable pure-logic pieces — classification, decision functions, subprocess-output parsing. I/O paths (zbus dispatch, process spawning, filesystem copy, Dioxus UI) rely on manual verification per the spec. Each task flags which approach it uses.

---

### Task 1: Add zbus dependency and create empty focus module

**Files:**
- Modify: `Cargo.toml`
- Create: `src/injection/focus.rs`
- Modify: `src/injection/mod.rs`

**Approach:** Build-verify only. No tests yet.

- [ ] **Step 1: Add zbus dep**

Add under `[target.'cfg(not(target_os = "windows"))'.dependencies]` in `Cargo.toml`. If that section doesn't exist, add it just above the existing `[target.'cfg(target_os = "windows")'.dependencies]` block. If neither exists, add a plain top-level entry under `[dependencies]`:

```toml
zbus = "5"
```

- [ ] **Step 2: Create focus.rs with a stub**

Create `src/injection/focus.rs`:

```rust
#![cfg(not(target_os = "windows"))]

//! Focused-window identification via the Beamer GNOME Shell extension.
//!
//! On GNOME Wayland the compositor does not expose focused-window metadata
//! to unprivileged clients, so we ship a minimal Shell extension that
//! exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` over D-Bus.
//! This module is the client side of that interface.

/// Returns the focused window's app id, lowercased. Returns `None` if the
/// extension isn't installed/enabled, the D-Bus call fails, or no window
/// is focused. Every failure path is silent (debug-logged) so callers can
/// treat `None` as "unknown, fall back to defaults".
pub fn focused_app_id() -> Option<String> {
    None // filled in later tasks
}
```

- [ ] **Step 3: Wire module into injection/mod.rs**

Add after the existing `pub mod ydotool;` line (or equivalent module declarations):

```rust
#[cfg(not(target_os = "windows"))]
pub mod focus;
```

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished dev profile ...` with no errors. zbus should download and compile (first build only).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/injection/focus.rs src/injection/mod.rs
git commit -m "chore(linux): add zbus dependency and focus module scaffold"
```

---

### Task 2: Terminal classification (TDD)

**Files:**
- Modify: `src/injection/focus.rs`

**Approach:** TDD. Pure function, trivially testable.

- [ ] **Step 1: Write failing test**

Append to `src/injection/focus.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_app_ids_match_case_insensitive() {
        assert!(is_terminal("org.wezfurlong.wezterm"));
        assert!(is_terminal("Kitty"));
        assert!(is_terminal("FOOT"));
        assert!(is_terminal("org.gnome.Console"));
    }

    #[test]
    fn non_terminal_app_ids_do_not_match() {
        assert!(!is_terminal("firefox"));
        assert!(!is_terminal("code"));
        assert!(!is_terminal("org.mozilla.firefox"));
        assert!(!is_terminal(""));
        assert!(!is_terminal("thunderbird")); // contains "term"; must not match
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib focus 2>&1 | tail -15`
Expected: `error[E0425]: cannot find function 'is_terminal'`

- [ ] **Step 3: Implement is_terminal**

Add above the `#[cfg(test)]` block:

```rust
const TERMINAL_APP_IDS: &[&str] = &[
    "org.gnome.console",
    "org.gnome.terminal",
    "org.wezfurlong.wezterm",
    "dev.warp.warp",
    "com.mitchellh.ghostty",
    "kitty",
    "foot",
    "alacritty",
    "org.kde.konsole",
    "io.elementary.terminal",
    "org.xfce.terminal",
    "com.raggesilver.blackbox",
    "tilix",
];

/// Exact-match against a curated list, case-insensitive. No substring
/// heuristic — "thunderbird" contains "term" but is not a terminal.
pub fn is_terminal(app_id: &str) -> bool {
    let lower = app_id.to_ascii_lowercase();
    TERMINAL_APP_IDS.iter().any(|id| *id == lower)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib focus 2>&1 | tail -10`
Expected: `test result: ok. 2 passed`

- [ ] **Step 5: Commit**

```bash
git add src/injection/focus.rs
git commit -m "feat(linux): add terminal app_id classifier"
```

---

### Task 3: Result mapping (TDD)

**Files:**
- Modify: `src/injection/focus.rs`

**Approach:** TDD. Pure mapping from `Result<String, zbus::Error>` to `Option<String>` — the part that's unit-testable without a live bus.

- [ ] **Step 1: Write failing tests**

Add to the `tests` module:

```rust
#[test]
fn empty_string_maps_to_none() {
    let got = map_call_result(Ok(String::new()));
    assert_eq!(got, None);
}

#[test]
fn non_empty_string_is_lowercased() {
    let got = map_call_result(Ok("Org.WezFurlong.WezTerm".into()));
    assert_eq!(got.as_deref(), Some("org.wezfurlong.wezterm"));
}

#[test]
fn dbus_error_maps_to_none() {
    let err = zbus::Error::Failure("simulated".into());
    let got = map_call_result(Err(err));
    assert_eq!(got, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib focus 2>&1 | tail -15`
Expected: `cannot find function 'map_call_result'`

- [ ] **Step 3: Implement map_call_result**

Add above the tests module:

```rust
fn map_call_result(result: Result<String, zbus::Error>) -> Option<String> {
    match result {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s.to_ascii_lowercase()),
        Err(e) => {
            tracing::debug!("focus: D-Bus call failed: {}", e);
            None
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib focus 2>&1 | tail -10`
Expected: `test result: ok. 5 passed`

- [ ] **Step 5: Commit**

```bash
git add src/injection/focus.rs
git commit -m "feat(linux): add D-Bus result → Option mapping for focus"
```

---

### Task 4: zbus call + focused_app_id

**Files:**
- Modify: `src/injection/focus.rs`

**Approach:** Build + manual verify. Live D-Bus call is not unit-testable without significant mock infrastructure.

- [ ] **Step 1: Implement call_extension**

Add above `focused_app_id`:

```rust
use std::time::Duration;

fn call_extension() -> Result<String, zbus::Error> {
    let conn = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.gnome.Shell",
        "/app/beamer/FocusProvider",
        "app.beamer.FocusProvider",
    )?;
    // zbus 5 supports per-proxy method timeout via `.with_method_timeout()`
    // during build, or set_default_timeout on the underlying Connection.
    // Keeping call-level timeout short so a stuck Shell doesn't block paste.
    let _ = Duration::from_millis(100); // documented intent; adjust to zbus 5 API
    proxy.call("GetFocusedAppId", &())
}
```

Note: if zbus 5's blocking Proxy API doesn't expose a per-call timeout cleanly, keep the call as-is — the session D-Bus default timeout (~25 s) is the fallback. Paste hangs at that point would be catastrophic, so if no API exists, wrap the call in `std::thread::spawn` + `recv_timeout` as a second step before shipping. Flag this to the reviewer.

- [ ] **Step 2: Replace focused_app_id stub**

Replace the current stub body:

```rust
pub fn focused_app_id() -> Option<String> {
    map_call_result(call_extension())
}
```

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 4: Manual smoke test (no extension yet, so expect None)**

Add a temporary `fn main()` test or use `cargo test` with an `#[ignore]` manual-only test. Simpler: run Beamer with debug logging and check the log later once the extension is installed (Task 6).

For now, just confirm `cargo test --lib focus` still passes (the 5 existing unit tests; `focused_app_id` itself isn't tested live).

Run: `cargo test --lib focus 2>&1 | tail -10`
Expected: `test result: ok. 5 passed`

- [ ] **Step 5: Commit**

```bash
git add src/injection/focus.rs
git commit -m "feat(linux): implement focused_app_id D-Bus client"
```

---

### Task 5: Refactor resolve_use_shift_v to pure decision function (TDD)

**Files:**
- Modify: `src/injection/clipboard.rs`

**Approach:** TDD. Extract the string→bool decision into a pure function that takes `focused: Option<&str>`, test all branches, then wire.

- [ ] **Step 1: Write failing tests**

Append to `src/injection/clipboard.rs` (above any existing `#[cfg(test)]` if present — there isn't one now):

```rust
#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn explicit_ctrl_v_wins_over_focus() {
        assert!(!choose_use_shift_v("ctrl_v", Some("org.wezfurlong.wezterm")));
    }

    #[test]
    fn explicit_ctrl_shift_v_wins_over_focus() {
        assert!(choose_use_shift_v("ctrl_shift_v", Some("firefox")));
    }

    #[test]
    fn auto_on_terminal_picks_shift_v() {
        assert!(choose_use_shift_v("auto", Some("org.wezfurlong.wezterm")));
        assert!(choose_use_shift_v("auto", Some("kitty")));
    }

    #[test]
    fn auto_on_non_terminal_picks_ctrl_v() {
        assert!(!choose_use_shift_v("auto", Some("firefox")));
        assert!(!choose_use_shift_v("auto", Some("code")));
    }

    #[test]
    fn auto_unknown_focus_falls_back_to_shift_v() {
        assert!(choose_use_shift_v("auto", None));
    }

    #[test]
    fn unrecognised_setting_falls_back_to_shift_v() {
        assert!(choose_use_shift_v("nonsense", Some("firefox")));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib clipboard 2>&1 | tail -15`
Expected: `cannot find function 'choose_use_shift_v'`

- [ ] **Step 3: Add pure decision function**

Add `choose_use_shift_v` above the existing `resolve_use_shift_v`:

```rust
/// Pure decision function: given the configured setting and the currently
/// focused app id (or None if unknown), return true for Ctrl+Shift+V, false
/// for Ctrl+V. Extracted for testability — the side-effectful
/// `resolve_use_shift_v` is a thin wrapper that fetches the inputs.
#[cfg(not(target_os = "windows"))]
fn choose_use_shift_v(setting: &str, focused: Option<&str>) -> bool {
    match setting.to_ascii_lowercase().as_str() {
        "ctrl_v" | "ctrl+v" => false,
        "auto" => match focused {
            Some(app) if crate::injection::focus::is_terminal(app) => true,
            Some(_) => false,
            None => true, // unknown focus → safe default
        },
        _ => true, // "ctrl_shift_v", "ctrl+shift+v", anything unrecognised
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib clipboard 2>&1 | tail -10`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: Commit**

```bash
git add src/injection/clipboard.rs
git commit -m "refactor(linux): extract pure paste-shortcut decision fn"
```

---

### Task 6: Wire focused_app_id into resolve_use_shift_v

**Files:**
- Modify: `src/injection/clipboard.rs`

**Approach:** Rewrite the existing `resolve_use_shift_v` to use the pure function. Build-verify.

- [ ] **Step 1: Replace resolve_use_shift_v body**

Replace the current implementation (lines ~290-310) with:

```rust
#[cfg(not(target_os = "windows"))]
fn resolve_use_shift_v() -> bool {
    let setting = std::env::var("BEAMER_PASTE_SHORTCUT")
        .ok()
        .or_else(|| {
            crate::config::Config::load()
                .ok()
                .map(|c| c.injection.paste_shortcut)
        })
        .unwrap_or_else(|| "auto".into());

    let focused = crate::injection::focus::focused_app_id();
    let use_shift = choose_use_shift_v(&setting, focused.as_deref());

    let combo = if use_shift { "Ctrl+Shift+V" } else { "Ctrl+V" };
    match focused.as_deref() {
        Some(app) => tracing::info!("Clipboard: paste_shortcut={} focus={} → {}", setting, app, combo),
        None => tracing::info!("Clipboard: paste_shortcut={} focus=unknown → {}", setting, combo),
    }
    use_shift
}
```

Also update the doc comment above it:

```rust
/// Decide which paste keystroke to send. Precedence:
///   1. BEAMER_PASTE_SHORTCUT env var ("ctrl_v" | "ctrl_shift_v" | "auto")
///   2. injection.paste_shortcut in config.toml
///   3. Default: "auto" — queries the Beamer GNOME focus helper extension
///      (if installed and enabled) to pick per-app. Falls back to Ctrl+Shift+V
///      when the extension is absent or the call fails.
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build. Tests from Task 5 still pass (`cargo test --lib clipboard`).

- [ ] **Step 3: Commit**

```bash
git add src/injection/clipboard.rs
git commit -m "feat(linux): query GNOME focus helper in paste-shortcut resolution"
```

---

### Task 7: Flip the config default to "auto"

**Files:**
- Modify: `src/config/mod.rs`

**Approach:** Single-line change. The string was already accepted by `resolve_use_shift_v` so this is safe — existing configs keep working.

- [ ] **Step 1: Edit default_paste_shortcut**

Find:

```rust
fn default_paste_shortcut() -> String { "ctrl_shift_v".into() }
```

Replace with:

```rust
fn default_paste_shortcut() -> String { "auto".into() }
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add src/config/mod.rs
git commit -m "feat(linux): default paste_shortcut to auto"
```

---

### Task 8: Create the GNOME extension files

**Files:**
- Create: `extension/beamer-focus@beamer.app/metadata.json`
- Create: `extension/beamer-focus@beamer.app/extension.js`

**Approach:** Manual verification via `gdbus call` after a local install. No automated test.

- [ ] **Step 1: Write metadata.json**

Create `extension/beamer-focus@beamer.app/metadata.json`:

```json
{
  "uuid": "beamer-focus@beamer.app",
  "name": "Beamer Focus Helper",
  "description": "Exposes the focused window's app id over D-Bus so Beamer can pick the correct paste shortcut per-app. Bundled with the Beamer dictation app.",
  "shell-version": ["48", "49", "50"],
  "url": "https://github.com/berkley/Beamer",
  "version": 1
}
```

- [ ] **Step 2: Write extension.js**

Create `extension/beamer-focus@beamer.app/extension.js`:

```javascript
import Gio from 'gi://Gio';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

const DBUS_XML = `
<node>
  <interface name="app.beamer.FocusProvider">
    <method name="GetFocusedAppId">
      <arg type="s" direction="out" name="app_id"/>
    </method>
  </interface>
</node>`;

export default class BeamerFocusExtension extends Extension {
    enable() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(DBUS_XML, this);
        this._dbus.export(Gio.DBus.session, '/app/beamer/FocusProvider');
    }

    disable() {
        if (this._dbus) {
            this._dbus.unexport();
            this._dbus = null;
        }
    }

    GetFocusedAppId() {
        const win = global.display.focus_window;
        if (!win) return '';
        const gtkId = win.get_gtk_application_id?.();
        if (gtkId && gtkId.length > 0) return gtkId;
        const wmClass = win.get_wm_class?.();
        return wmClass ? wmClass.toLowerCase() : '';
    }
}
```

- [ ] **Step 3: Local install for manual test**

```bash
EXT_DIR="$HOME/.local/share/gnome-shell/extensions/beamer-focus@beamer.app"
mkdir -p "$EXT_DIR"
cp extension/beamer-focus@beamer.app/*.json extension/beamer-focus@beamer.app/*.js "$EXT_DIR/"
gnome-extensions enable beamer-focus@beamer.app
```

After this, **log out and back in** (GNOME Shell on Wayland does not hot-load new extensions). Alternatively restart GNOME Shell on X11 with Alt+F2 → `r`, but Ubuntu 26.04 is Wayland.

- [ ] **Step 4: Verify D-Bus interface via gdbus**

Focus a terminal window, then in another terminal:

```bash
gdbus call --session --dest org.gnome.Shell --object-path /app/beamer/FocusProvider --method app.beamer.FocusProvider.GetFocusedAppId
```

Expected: something like `('org.wezfurlong.wezterm',)` or the focused app's id. Focus Firefox and repeat — expected `('firefox',)` or `('org.mozilla.firefox',)`.

- [ ] **Step 5: Verify Beamer now sees it**

Run Beamer with debug logging: `RUST_LOG=beamer=debug cargo run`. Dictate into a terminal and into a non-terminal app. Log should show:

```
Clipboard: paste_shortcut=auto focus=org.wezfurlong.wezterm → Ctrl+Shift+V
Clipboard: paste_shortcut=auto focus=firefox → Ctrl+V
```

- [ ] **Step 6: Commit**

```bash
git add extension/
git commit -m "feat(linux): add GNOME Shell extension for focused-window identification"
```

---

### Task 9: Install helper — scaffolding + status probe (partial TDD)

**Files:**
- Create: `src/install/mod.rs`
- Create: `src/install/gnome_extension.rs`
- Modify: `src/main.rs`

**Approach:** TDD for the `gnome-extensions list --details` parser. Process spawn and filesystem copy are manual-verified.

- [ ] **Step 1: Create module root**

Create `src/install/mod.rs`:

```rust
#![cfg(not(target_os = "windows"))]

pub mod gnome_extension;
```

Add to `src/main.rs` near the other `mod` declarations:

```rust
#[cfg(not(target_os = "windows"))]
mod install;
```

- [ ] **Step 2: Create gnome_extension.rs with status types and failing test**

Create `src/install/gnome_extension.rs`:

```rust
#![cfg(not(target_os = "windows"))]

//! Install / enable / disable helper for the bundled Beamer Focus Helper
//! GNOME Shell extension.

use anyhow::Result;

pub const EXTENSION_UUID: &str = "beamer-focus@beamer.app";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Extension present on disk and enabled in GNOME.
    Enabled,
    /// Present on disk but disabled.
    Disabled,
    /// Not installed.
    NotInstalled,
}

/// Parses a line from `gnome-extensions list --details`. Output is
/// whitespace-indented key/value lines per extension, e.g.:
///
/// ```text
/// beamer-focus@beamer.app
///   Name: Beamer Focus Helper
///   State: ACTIVE
///   Type: PER_USER
/// ```
///
/// We only care about the `State:` line for our UUID. Returns `None` if the
/// UUID is not mentioned.
fn parse_status(output: &str, uuid: &str) -> Option<Status> {
    let mut in_block = false;
    for line in output.lines() {
        let trimmed = line.trim_end();
        if trimmed == uuid {
            in_block = true;
            continue;
        }
        if in_block {
            if !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
                // Next extension's UUID line — we left the block without a State.
                return Some(Status::Disabled);
            }
            if let Some(state) = trimmed.trim().strip_prefix("State:") {
                return Some(match state.trim() {
                    "ACTIVE" => Status::Enabled,
                    _ => Status::Disabled,
                });
            }
        }
    }
    if in_block { Some(Status::Disabled) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_finds_active() {
        let out = "beamer-focus@beamer.app\n  Name: Beamer Focus Helper\n  State: ACTIVE\n  Type: PER_USER\n";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), Some(Status::Enabled));
    }

    #[test]
    fn parse_status_finds_inactive() {
        let out = "beamer-focus@beamer.app\n  Name: Beamer Focus Helper\n  State: INACTIVE\n";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), Some(Status::Disabled));
    }

    #[test]
    fn parse_status_not_listed() {
        let out = "other-ext@example.com\n  Name: Other\n  State: ACTIVE\n";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), None);
    }

    #[test]
    fn parse_status_handles_multiple_extensions() {
        let out = "\
other-ext@example.com
  State: ACTIVE
beamer-focus@beamer.app
  State: INACTIVE
third-ext@example.com
  State: ACTIVE
";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), Some(Status::Disabled));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib gnome_extension 2>&1 | tail -10`
Expected: `test result: ok. 4 passed`

- [ ] **Step 4: Commit**

```bash
git add src/install/ src/main.rs
git commit -m "feat(linux): scaffold GNOME extension install helper + status parser"
```

---

### Task 10: Install helper — process integration (status probe + install + uninstall)

**Files:**
- Modify: `src/install/gnome_extension.rs`

**Approach:** Manual verification — process spawn and filesystem operations aren't unit-testable without shimming `std::process::Command`.

- [ ] **Step 1: Add the source-path resolver**

Append to `src/install/gnome_extension.rs`:

```rust
use std::path::PathBuf;
use std::process::Command;

/// Where on disk the extension's files live, to copy from at install time.
/// Resolution order:
///   1. `$BEAMER_EXTENSION_DIR` env var (dev / packaging override)
///   2. `<exe_dir>/../share/beamer/extension/beamer-focus@beamer.app/` (FHS-packaged)
///   3. `<exe_dir>/extension/beamer-focus@beamer.app/` (portable / dev cwd)
///   4. `./extension/beamer-focus@beamer.app/` (cargo-run from repo root)
pub fn locate_source_dir() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("BEAMER_EXTENSION_DIR") {
        let p = PathBuf::from(env);
        if p.join("metadata.json").exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let fhs = dir.join("../share/beamer/extension").join(EXTENSION_UUID);
            if fhs.join("metadata.json").exists() {
                return Some(fhs);
            }
            let portable = dir.join("extension").join(EXTENSION_UUID);
            if portable.join("metadata.json").exists() {
                return Some(portable);
            }
        }
    }
    let cwd = PathBuf::from("extension").join(EXTENSION_UUID);
    if cwd.join("metadata.json").exists() {
        return Some(cwd);
    }
    None
}

fn target_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join(".local/share/gnome-shell/extensions")
        .join(EXTENSION_UUID))
}
```

- [ ] **Step 2: Add the status probe**

Append:

```rust
/// Query GNOME for the extension's current state. Returns `NotInstalled`
/// if the `gnome-extensions` tool isn't on PATH or the UUID isn't listed.
pub fn status() -> Status {
    let out = match Command::new("gnome-extensions")
        .arg("list")
        .arg("--details")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Status::NotInstalled,
    };
    let s = String::from_utf8_lossy(&out.stdout);
    parse_status(&s, EXTENSION_UUID).unwrap_or(Status::NotInstalled)
}
```

- [ ] **Step 3: Add install**

Append:

```rust
/// Copy the bundled extension into the user's GNOME extensions dir and
/// enable it. Idempotent — re-running after a prior install updates the
/// files.
pub fn install() -> Result<()> {
    let src = locate_source_dir()
        .ok_or_else(|| anyhow::anyhow!("extension source directory not found; set BEAMER_EXTENSION_DIR"))?;
    let dst = target_dir()?;
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(&src)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    let out = Command::new("gnome-extensions")
        .arg("enable")
        .arg(EXTENSION_UUID)
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("gnome-extensions enable failed: {}", err.trim());
    }
    Ok(())
}
```

- [ ] **Step 4: Add uninstall**

Append:

```rust
/// Disable and remove the extension.
pub fn uninstall() -> Result<()> {
    let _ = Command::new("gnome-extensions")
        .arg("disable")
        .arg(EXTENSION_UUID)
        .output();
    let dst = target_dir()?;
    if dst.exists() {
        std::fs::remove_dir_all(&dst)?;
    }
    Ok(())
}
```

- [ ] **Step 5: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 6: Manual smoke test**

```bash
# Remove the manual install from Task 8 so we can verify programmatic install
gnome-extensions disable beamer-focus@beamer.app 2>/dev/null || true
rm -rf ~/.local/share/gnome-shell/extensions/beamer-focus@beamer.app

# Build a tiny exploratory binary; simpler: add a unit test marked #[ignore]
cat > /tmp/ext_test.rs <<'EOF'
// ignore — run manually with: rustc --edition 2021 ...
EOF
```

Easier approach: just run Beamer itself after wiring the UI in Task 11 and click Install there. For this task, confirm `cargo test --lib gnome_extension` still passes (4 unit tests — they cover the parser only; install/uninstall are exercised via the UI).

Run: `cargo test --lib gnome_extension 2>&1 | tail -10`
Expected: `test result: ok. 4 passed`

- [ ] **Step 7: Commit**

```bash
git add src/install/gnome_extension.rs
git commit -m "feat(linux): implement GNOME extension install/status/uninstall"
```

---

### Task 11: Settings UI — Auto option + focus helper subsection

**Files:**
- Modify: `src/ui/settings/injection_card.rs`

**Approach:** Manual verification — Dioxus UI isn't unit-tested in this repo.

- [ ] **Step 1: Add "Auto" to the paste-shortcut Select options**

Find the Select at lines ~133-146 in `src/ui/settings/injection_card.rs`. Replace:

```rust
Select {
    value: props.paste_shortcut.clone(),
    options: vec![
        ("ctrl_shift_v".into(), "Ctrl+Shift+V (default)".into()),
        ("ctrl_v".into(), "Ctrl+V".into()),
    ],
    onchange: move |v: String| props.on_paste_shortcut_change.call(v),
}
```

With:

```rust
Select {
    value: props.paste_shortcut.clone(),
    options: vec![
        ("auto".into(), "Auto (recommended)".into()),
        ("ctrl_shift_v".into(), "Ctrl+Shift+V".into()),
        ("ctrl_v".into(), "Ctrl+V".into()),
    ],
    onchange: move |v: String| props.on_paste_shortcut_change.call(v),
}
```

Also update the default in the props declaration:

```rust
#[props(default = String::from("auto"))]
pub paste_shortcut: String,
```

And update the comment block above the div:

```rust
// Paste-shortcut override for the clipboard backend (Linux only).
// "Auto" queries the GNOME focus helper extension (see section below)
// to pick Ctrl+V or Ctrl+Shift+V per-app. If the helper isn't installed
// we fall back to Ctrl+Shift+V — the universal terminal paste that also
// degrades to "paste plain text" in most other apps.
```

- [ ] **Step 2: Add the GNOME focus helper subsection**

Append inside the same `Card { ... }` block, after the paste-shortcut div (just before the closing `}` of `Card`):

```rust
// GNOME focus helper — only show on GNOME Wayland
{
    let is_gnome_wayland = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_ascii_uppercase().contains("GNOME"))
        .unwrap_or(false)
        && std::env::var("XDG_SESSION_TYPE").ok().as_deref() == Some("wayland");

    if is_gnome_wayland {
        let status = use_hook(|| {
            use_signal(|| crate::install::gnome_extension::status())
        });
        let refresh = move || status.set(crate::install::gnome_extension::status());

        let (label, action): (&str, Option<&str>) = match status() {
            crate::install::gnome_extension::Status::Enabled =>
                ("GNOME focus helper: Active", Some("Remove")),
            crate::install::gnome_extension::Status::Disabled =>
                ("GNOME focus helper: Installed but disabled — log out and back in", Some("Remove")),
            crate::install::gnome_extension::Status::NotInstalled =>
                ("Install GNOME focus helper for app-aware pasting", Some("Install")),
        };

        rsx! {
            div { class: "card-row",
                span { class: "card-label", "{label}" }
                if let Some(btn) = action {
                    button {
                        class: "btn-small",
                        onclick: move |_| {
                            let result = match status() {
                                crate::install::gnome_extension::Status::NotInstalled =>
                                    crate::install::gnome_extension::install(),
                                _ => crate::install::gnome_extension::uninstall(),
                            };
                            if let Err(e) = result {
                                tracing::warn!("GNOME extension action failed: {}", e);
                            }
                            refresh();
                        },
                        "{btn}"
                    }
                }
            }
        }
    } else {
        rsx! { }
    }
}
```

Note: Dioxus 0.7 `use_signal`/`use_hook` syntax — if your version of Dioxus uses different hook names, substitute the equivalent (the goal is a reactive status that refreshes after button click). Check existing hook usage at the top of this file (`use_hook(|| crate::injection::check_availability())`) for the local convention.

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build. Dioxus compile errors likely need minor hook-API adjustments — if the `use_signal` call doesn't compile, replace with `use_state` or the pattern used elsewhere in the settings cards. (See `src/ui/settings/*.rs` for examples.)

- [ ] **Step 4: Manual verification**

Run `cargo run`. Open the settings window, go to the Injection card:

- If the extension is not installed → a row appears: "Install GNOME focus helper for app-aware pasting" + Install button. Click it. GNOME renders its native "Allow extension to be enabled?" dialog. Accept. Row should update to "Active" (may require log-out/log-in; if so, the row says so).
- If already enabled → row shows "Active" with a Remove button.
- On X11 session or non-GNOME desktop → the row is hidden entirely.
- The paste-shortcut dropdown shows "Auto (recommended)" as the first option.

- [ ] **Step 5: Commit**

```bash
git add src/ui/settings/injection_card.rs
git commit -m "feat(ui): surface GNOME focus helper install flow in settings"
```

---

### Task 12: Documentation

**Files:**
- Modify: `agent_docs/text_injection.md`

**Approach:** Docs only. No tests or build.

- [ ] **Step 1: Add a new section under the existing "Clipboard" description**

Open `agent_docs/text_injection.md`. Find the section that currently describes the Linux clipboard backend and ydotool paste. After that section add:

```markdown
### Focus-aware paste-shortcut detection (GNOME)

On GNOME Wayland, the compositor does not expose focused-window metadata to
unprivileged clients. To pick Ctrl+V vs Ctrl+Shift+V per-app, Beamer bundles
a minimal GNOME Shell extension (`extension/beamer-focus@beamer.app/`) that
exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` on the session bus
via the `org.gnome.Shell` name.

The Rust side (`src/injection/focus.rs`) queries this method on each paste
via zbus and classifies the returned app id against a curated terminal
list. Terminals → Ctrl+Shift+V; anything else → Ctrl+V. Any failure
(extension missing, disabled, D-Bus timeout, unknown app) falls back to
Ctrl+Shift+V — the current pre-feature default — so users who never
install the extension see no regression.

**Install:** Settings → Text Injection → "Install GNOME focus helper".
Button copies the extension files to
`~/.local/share/gnome-shell/extensions/beamer-focus@beamer.app/` and runs
`gnome-extensions enable`. GNOME renders its own native "enable extension?"
dialog — that's the privilege-grant moment.

**After install on Wayland**, the user must log out and back in once.
GNOME Shell does not hot-load new extensions on Wayland. The Settings card
hints at this when it detects the extension installed-but-not-yet-loaded
state.

**Terminal list** lives at `src/injection/focus.rs::TERMINAL_APP_IDS`.
To add a terminal, PR-append the app id (lowercase, exact match — no
substring heuristic).

**Supported GNOME versions:** 48, 49, 50. Older versions fall through the
feature-detection guard (`XDG_CURRENT_DESKTOP` + `XDG_SESSION_TYPE`) and
get the pre-feature default Ctrl+Shift+V. Non-GNOME desktops (KDE, wlroots)
same treatment — adding support per-compositor is a future item.
```

- [ ] **Step 2: Commit**

```bash
git add agent_docs/text_injection.md
git commit -m "docs(linux): document GNOME focus helper extension and install flow"
```

---

### Task 13: Packaging — ensure extension dir ships with release builds

**Files:**
- Inspect: existing packaging config (look for `installer/`, `dist/`, `Cargo.toml` `[package.metadata]`, or shell scripts).
- Modify: whichever of those packages the binary.

**Approach:** Manual — the exact change depends on the current packaging path, which wasn't inspected during design.

- [ ] **Step 1: Locate packaging config**

Run: `ls installer/ dist/ scripts/ 2>&1; grep -r "extension" Cargo.toml 2>&1 || true`

Look for any tarball / AppImage / deb build script. Beamer has an auto-update pipeline per recent commits; look for a release workflow under `.github/workflows/` too if present.

- [ ] **Step 2: Add the extension directory to whatever the release bundles**

Three common patterns:

- **Tarball / AppImage script:** add a `cp -r extension/beamer-focus@beamer.app $OUT/share/beamer/extension/` step.
- **cargo-deb:** add an `[package.metadata.deb.assets]` entry mapping `extension/beamer-focus@beamer.app/*` to `/usr/share/beamer/extension/beamer-focus@beamer.app/`.
- **Manual copy in release README:** document the step if no automation exists yet.

The Rust install helper (`locate_source_dir`) already checks
`<exe_dir>/../share/beamer/extension/` which matches the FHS path. Align
the packaging to that target.

- [ ] **Step 3: Verify**

Build the release artifact locally and unpack it (or simulate for a script-based build). Confirm `extension/beamer-focus@beamer.app/metadata.json` is present at the expected path.

- [ ] **Step 4: Commit**

```bash
git add <packaging files>
git commit -m "build(linux): include GNOME focus helper extension in release artifacts"
```

If packaging doesn't exist yet or lives outside the repo, skip this task with a note in `todo.md`.

---

## Self-review

**Spec coverage:** Every spec section has a task:
- Component architecture → Tasks 1, 4, 8, 9, 10, 11 (all files created/edited)
- D-Bus surface → Task 8 (metadata, extension.js), Task 4 (client)
- Terminal matching → Task 2
- Install UX / fallback → Tasks 10, 11 + Task 6 (fallback in decision fn via Task 5)
- GNOME 50 specifics → Task 8 (shell-version, ESM), Task 1 (zbus 5), Task 11 (feature-detect env)

**Placeholder scan:** Two intentional flags (each clearly marked):
- Task 4 Step 1 notes that zbus 5's per-call timeout API may differ from the documented intent — implementer adjusts if needed.
- Task 11 Step 3 notes that Dioxus 0.7 hook names may need substituting from `use_signal` to whatever this file uses — implementer checks local convention.
- Task 13 is inherently discovery-based (packaging wasn't inspected during design). That's flagged explicitly, not hidden as a placeholder.

No vague "add error handling" / "handle edge cases" / "write tests for the above" steps. Every code step shows the actual code.

**Type consistency:** `Status` enum has three variants (`Enabled`, `Disabled`, `NotInstalled`) — used consistently in Tasks 9, 10, 11. `EXTENSION_UUID` constant referenced by string literal `"beamer-focus@beamer.app"` in install paths and JSON metadata — all match. `choose_use_shift_v` signature `(setting: &str, focused: Option<&str>) -> bool` consistent between Task 5 definition and Task 6 caller.

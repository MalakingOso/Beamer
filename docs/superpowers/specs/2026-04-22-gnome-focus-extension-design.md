# GNOME Focus-Aware Paste Shortcut

**Status:** Approved 2026-04-22
**Target platform:** Ubuntu 26.04 / GNOME 50 Wayland (back-compat to GNOME 48)

## Problem

On Linux, Beamer's clipboard injection backend synthesises a paste shortcut via
ydotool after setting the clipboard. The shortcut is either Ctrl+V or
Ctrl+Shift+V, currently picked from a config field with no per-app awareness.
The wrong choice silently no-ops: terminals (WezTerm, Ghostty, Warp, Kitty,
etc.) ignore Ctrl+V, while some editors (VS Code, JetBrains) hijack
Ctrl+Shift+V for other commands. ydotool reports success in both cases
because the keystroke was synthesised correctly — the failure is downstream
at the app layer, invisible to us.

The current default is Ctrl+Shift+V, chosen because it's the universal
terminal paste and degrades to "paste plain text" in most other apps. It
still fails in editor shortcuts and in apps that don't recognise it at all.

## Constraint

Mutter on GNOME 50 does not advertise `ext_foreign_toplevel_list_v1` or
`zwlr_foreign_toplevel_manager_v1` to unprivileged Wayland clients, and
`org.gnome.Shell.Introspect.GetWindows` returns `AccessDenied` from outside
the shell process. The only supported escape valve is a GNOME Shell
extension, which runs inside the shell and therefore passes the privilege
check by construction.

## Solution

Bundle a minimal GNOME Shell extension with Beamer that exposes the focused
window's application identifier over D-Bus. The Rust side queries it on each
paste and picks the correct shortcut per-app. All failure modes degrade to
the current Ctrl+Shift+V default — no regression for users who don't install.

## Component architecture

| Path | Role |
|---|---|
| `extension/beamer-focus@beamer.app/metadata.json` | Extension manifest. Declares `shell-version: ["48", "49", "50"]`. |
| `extension/beamer-focus@beamer.app/extension.js` | ~40 LOC. On `enable()` registers a D-Bus object; on `disable()` unregisters. Reads `global.display.focus_window` on each call — no polling, no signal subscriptions. |
| `src/injection/focus.rs` (new) | zbus blocking client. Public surface: `pub fn focused_app_id() -> Option<String>`. Short timeout (100 ms). Any D-Bus error → `None`. |
| `src/injection/clipboard.rs` (edit) | `resolve_use_shift_v()` gains a real `"auto"` branch. The value was already accepted as a legacy alias mapping to Ctrl+Shift+V; we upgrade its meaning to "query the extension, fall back to Ctrl+Shift+V." |
| `src/config/mod.rs` (edit) | `default_paste_shortcut()` → `"auto"`. |
| `src/ui/settings/injection_card.rs` (edit) | New subsection "GNOME focus helper" with a status line and Install/Enable/Remove buttons. |
| `src/install/mod.rs` + `src/install/gnome_extension.rs` (new, ~80 LOC) | File copy to `~/.local/share/gnome-shell/extensions/`, `gnome-extensions enable/disable/list` subprocess wrapper, status probe. Idempotent install. Source path resolution: check `$BEAMER_EXTENSION_DIR` env var first (dev), else `<exe_dir>/../share/beamer/extension/beamer-focus@beamer.app/` (packaged), else `<exe_dir>/extension/beamer-focus@beamer.app/` (portable). |
| `agent_docs/text_injection.md` (edit) | Document the detection chain and install flow. |
| `Cargo.toml` (edit) | Add `zbus = "5"`. |
| Packaging (edit) | Ship the `extension/beamer-focus@beamer.app/` directory alongside the release binary so the install helper can copy from a known location. |

## D-Bus surface

Minimal single-string method.

```
Bus name:    org.gnome.Shell
Object path: /app/beamer/FocusProvider
Interface:   app.beamer.FocusProvider
Method:      GetFocusedAppId() -> s
```

Return value is the best identifier for the currently focused toplevel:

1. `Meta.Window.get_gtk_application_id()` if non-empty (preferred — native
   Wayland apps and GTK apps).
2. Otherwise `Meta.Window.get_wm_class()` lowercased (XWayland apps,
   everything else).
3. Otherwise the empty string (no window focused, or focused window has no
   identifier).

The empty-string return is semantically identical to a D-Bus error for the
client: treat as "don't know, fall back."

Bus name is `org.gnome.Shell` (we export an object path inside the Shell's
own bus name — standard extension practice). No bus-name ownership dance
required on the extension side.

## Terminal matching

Exact-match case-insensitive lookup against a curated list. No substring
heuristic.

```rust
const TERMINAL_APP_IDS: &[&str] = &[
    "org.gnome.console", "org.gnome.terminal",
    "org.wezfurlong.wezterm", "dev.warp.warp",
    "com.mitchellh.ghostty",
    "kitty", "foot", "alacritty",
    "org.kde.konsole",
    "io.elementary.terminal", "org.xfce.terminal",
    "com.raggesilver.blackbox", "tilix",
];
```

If the focused app id matches → Ctrl+Shift+V. Otherwise → Ctrl+V. If the
extension returns empty or the D-Bus call fails → Ctrl+Shift+V (current
default behaviour).

Substring matching on `"term"` or `"console"` was considered and rejected:
risks false positives against names like Thunderbird. Users with unlisted
terminals hit the Ctrl+Shift+V fallback — the current behaviour — so there
is no regression. A future `injection.terminal_app_ids = [...]` config
override can extend the list without code changes; not shipping in v1.

## Install UX and fallback behaviour

Settings card "GNOME focus helper" lives inside the existing injection card.
It shows one of five states:

| State | Display | Action |
|---|---|---|
| Installed & enabled | "GNOME focus helper: Active" | Remove button |
| Installed but disabled | "Disabled — click to enable" | Enable button (runs `gnome-extensions enable`) |
| Not installed, on GNOME Wayland | "Install GNOME focus helper for app-aware pasting" | Install button |
| Not on GNOME, or not Wayland | Section hidden entirely | N/A |
| Installed, enabled, but D-Bus call fails | Runtime-only — no UI change | Silent fall back per call; `tracing::debug!` the error |

The Install button copies the bundled `extension/beamer-focus@beamer.app/`
directory to `~/.local/share/gnome-shell/extensions/beamer-focus@beamer.app/`,
then runs `gnome-extensions enable beamer-focus@beamer.app`. GNOME renders
its own native "enable extension?" confirmation dialog — that dialog is the
privilege-grant moment the user consents to.

After enable on Wayland, existing GNOME Shell instances do not hot-load new
extensions (GNOME's long-standing Wayland limitation). The settings card
shows a hint: **"Log out and back in to activate"** when `gnome-extensions
list` shows the extension present but the D-Bus method is unreachable.

Install is user-initiated via the settings button. Beamer does not auto-
install on first launch. Silent install without explicit consent would
defeat the purpose of the privilege-grant ceremony.

### Failure-mode guarantee

Every failure mode in the chain — extension not installed, extension
disabled, extension present but Shell not yet reloaded, D-Bus call timeout,
unknown app id — falls back to the current Ctrl+Shift+V default. Users who
never install the extension see exactly today's behaviour. There is no
regression path.

## GNOME 50 / Ubuntu 26.04 specifics

- `metadata.json` declares `shell-version: ["48", "49", "50"]`. GNOME 45
  introduced the current extension class base; 48 is the first version
  worth targeting as a floor. Ubuntu 26.04 ships GNOME 50.
- `extension.js` uses ESM: `import GObject from 'gi://GObject'` and
  `import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js'`.
- D-Bus export via `Gio.DBusExportedObject.wrapJSObject(xml, this)` and
  `.export(Gio.DBus.session, '/app/beamer/FocusProvider')`. Unexport in
  `disable()`.
- Mutter on GNOME 50 exposes `global.display.focus_window` and
  `Meta.Window.get_gtk_application_id()` / `.get_wm_class()` as stable APIs.
- Rust: `zbus = "5"` (current major). Calls go through `tokio::task::
  spawn_blocking` per the project's CLAUDE.md rule, same pattern as UIA
  calls on Windows.
- Installed extension path: `~/.local/share/gnome-shell/extensions/beamer-focus@beamer.app/`.
- Install helper checks `$XDG_CURRENT_DESKTOP` contains `GNOME` and
  `$XDG_SESSION_TYPE == "wayland"` before showing the settings section.
  The feature is meaningless on X11 (Beamer already has `xdotool`-based
  options there historically, though not currently in the chain) and on
  non-GNOME desktops.

## Out of scope

- KDE / wlroots / Sway / Hyprland support. Each uses a different focus
  API; KDE has KWin scripts, wlroots has `zwlr_foreign_toplevel_manager_v1`.
  Addressable later with per-desktop plugins behind the same
  `focused_app_id()` interface.
- User-editable terminal list in config. Deferred; add when a real user
  asks for it.
- Automatic re-install after GNOME Shell updates break compatibility.
  Requires the user to rerun Install from settings.
- Streaming paste (injecting as transcription arrives). Separate feature.

## Testing strategy

- **Extension:** manual install + `gdbus call` from a second shell to
  verify the D-Bus method returns the expected string as focus changes.
- **Rust client:** unit test against a mock zbus server that returns
  canned strings including empty, a terminal id, a non-terminal id, and
  an error. Verify `focused_app_id()` maps all error paths to `None`.
- **`resolve_use_shift_v()`:** unit test the branching on mocked
  `focused_app_id()` values across each case.
- **End-to-end:** manual — dictate into each of {GNOME Console, WezTerm,
  Firefox, VS Code, Ghostty} and confirm paste lands correctly. The
  automated layer can't cover real keystroke synthesis reliably.

## Open questions resolved during design

- **Which dep for D-Bus?** `zbus = "5"` — pure Rust, no system libdbus,
  async+blocking APIs.
- **Where does Install UI live?** Inside `injection_card.rs`, not a new
  top-level card. It's about paste reliability.
- **Bundle vs recommend external extension?** Bundle. Third-party
  dependency would make maintenance invisible and fragile.
- **D-Bus surface shape?** Single string `GetFocusedAppId() -> s`. Dict
  return considered; YAGNI.
- **Terminal match strategy?** Exact list only; substring rejected due
  to false-positive risk.

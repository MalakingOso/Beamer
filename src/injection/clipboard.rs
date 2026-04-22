use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use arboard::Clipboard;

pub struct ClipboardBackend;

impl InjectionBackend for ClipboardBackend {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    fn display_name(&self) -> &'static str {
        "Clipboard (Ctrl+V paste)"
    }

    fn available(&self) -> Result<(), String> {
        // Quick check: can we create a clipboard handle?
        Clipboard::new().map(|_| ()).map_err(|e| format!("Clipboard unavailable: {}", e))
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let auto_pasted = inject_via_clipboard(text)?;

        #[cfg(target_os = "windows")]
        let target_info = "via Ctrl+V paste".to_string();
        #[cfg(not(target_os = "windows"))]
        let target_info = if auto_pasted {
            "via ydotool paste".to_string()
        } else {
            "set on clipboard — paste manually".to_string()
        };
        let _ = auto_pasted;

        Ok(InjectionResult {
            method: "Clipboard".into(),
            target_info,
        })
    }
}

/// Sets clipboard text and attempts to paste. On Windows, always sends Ctrl+V via
/// SendInput. On Linux, tries ydotool (kernel uinput — no portal prompt) and falls
/// back to manual paste if ydotool is unavailable. Returns `true` if the text was
/// auto-pasted, `false` if the user needs to paste it manually. The previous
/// clipboard is only restored when the paste actually happened.
fn inject_via_clipboard(text: &str) -> Result<bool> {
    let mut clipboard = Clipboard::new()?;
    let saved = clipboard.get_text().ok();

    // --- Step 1: Set the clipboard ---
    tracing::info!("Clipboard: setting {} bytes of text", text.len());

    #[cfg(not(target_os = "windows"))]
    set_clipboard_linux(text, &mut clipboard)?;

    #[cfg(target_os = "windows")]
    {
        if clipboard.set_text(text).is_err() {
            anyhow::bail!("Failed to set clipboard text");
        }
    }

    #[cfg(target_os = "windows")]
    {
        std::thread::sleep(std::time::Duration::from_millis(80));
        tracing::info!("Clipboard: sending Ctrl+V");
        send_ctrl_v()?;
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Some(saved_text) = saved {
            let _ = clipboard.set_text(saved_text);
        }
        Ok(true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Give the compositor a tick to settle after the clipboard write
        // before we fire the keystroke.
        std::thread::sleep(std::time::Duration::from_millis(80));

        if try_ydotool_paste() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // Only restore the previous clipboard if the paste actually happened —
            // otherwise we'd overwrite the transcript before the user could paste.
            if let Some(saved_text) = saved {
                let _ = clipboard.set_text(saved_text);
            }
            Ok(true)
        } else {
            tracing::info!(
                "Clipboard: ydotool unavailable — text set on clipboard, press Ctrl+V (or Ctrl+Shift+V in a terminal) to paste. Clipboard will not auto-restore."
            );
            // Deliberately do NOT restore the previous clipboard: the user
            // hasn't pasted yet and we would clobber the transcript.
            let _ = clipboard; // keep handle alive until wl-copy has served the selection
            Ok(false)
        }
    }
}

/// Set clipboard on Linux, with wl-copy fallback and verification.
#[cfg(not(target_os = "windows"))]
fn set_clipboard_linux(text: &str, clipboard: &mut Clipboard) -> Result<()> {
    let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    // Try arboard first
    let arboard_ok = clipboard.set_text(text).is_ok();
    if arboard_ok {
        tracing::debug!("Clipboard: arboard set_text succeeded");
    }

    if is_wayland {
        // On Wayland, verify the clipboard was actually set
        std::thread::sleep(std::time::Duration::from_millis(80));
        if !verify_clipboard_contains(text) {
            tracing::warn!("Clipboard: arboard reported success but verification failed, trying wl-copy");
            set_clipboard_wl_copy(text)?;
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !verify_clipboard_contains(text) {
                anyhow::bail!(
                    "Clipboard text not set — clipboard access may be blocked by compositor"
                );
            }
        }
    } else if !arboard_ok {
        anyhow::bail!("Failed to set clipboard text via arboard (X11)");
    }

    tracing::info!("Clipboard: text verified on clipboard");
    Ok(())
}

/// Verify the clipboard actually contains the expected text using wl-paste.
#[cfg(not(target_os = "windows"))]
fn verify_clipboard_contains(expected: &str) -> bool {
    match std::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
    {
        Ok(out) if out.status.success() => {
            let content = String::from_utf8_lossy(&out.stdout);
            if content == expected {
                true
            } else {
                tracing::debug!(
                    "Clipboard verification: expected {} bytes, got {} bytes",
                    expected.len(),
                    content.len()
                );
                false
            }
        }
        Ok(out) => {
            tracing::warn!("wl-paste exited with status {}", out.status);
            true // Can't verify, assume it worked
        }
        Err(e) => {
            tracing::warn!("wl-paste not available: {}", e);
            true // Can't verify, assume it worked
        }
    }
}

/// Set clipboard via wl-copy subprocess.
#[cfg(not(target_os = "windows"))]
fn set_clipboard_wl_copy(text: &str) -> Result<()> {
    let mut child = std::process::Command::new("wl-copy")
        .arg("--type")
        .arg("text/plain")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(text.as_bytes())?;
    }
    // Don't wait — wl-copy stays alive to serve clipboard requests
    tracing::debug!("Clipboard: wl-copy process spawned");
    Ok(())
}

#[cfg(target_os = "windows")]
fn send_ctrl_v() -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };

    let vk_control = VIRTUAL_KEY(0x11);
    let vk_v = VIRTUAL_KEY(0x56);

    let inputs = [
        make_key_input(vk_control, false),
        make_key_input(vk_v, false),
        make_key_input(vk_v, true),
        make_key_input(vk_control, true),
    ];

    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn make_key_input(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    key_up: bool,
) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    };

    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags = KEYEVENTF_KEYUP;
    }

    INPUT {
        r#type: INPUT_TYPE(1),
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// On Linux, attempts the paste shortcut via ydotool (kernel uinput — no portal
/// prompt). Picks Ctrl+V or Ctrl+Shift+V based on the configured `paste_shortcut`
/// (env var `BEAMER_PASTE_SHORTCUT` overrides config). Default is Ctrl+Shift+V:
/// it's the correct paste shortcut in every terminal, and in browsers/office apps
/// it degrades to "paste without formatting" — which is usually what you want for
/// a transcript. On GNOME Wayland, the bundled focus helper extension provides
/// per-app detection via D-Bus (see `resolve_use_shift_v`); other compositors
/// still fall back to the configured default.
///
/// Returns `true` if ydotool successfully sent the keystroke, `false` if ydotool
/// is unavailable or failed (caller should fall back to manual paste).
///
/// Linux evdev keycodes used:
///   29 = KEY_LEFTCTRL, 42 = KEY_LEFTSHIFT, 47 = KEY_V
/// Suffix: `:1` = key down, `:0` = key up.
#[cfg(not(target_os = "windows"))]
fn try_ydotool_paste() -> bool {
    let use_shift = resolve_use_shift_v();
    let (combo, args): (&str, Vec<&str>) = if use_shift {
        (
            "Ctrl+Shift+V",
            vec!["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"],
        )
    } else {
        (
            "Ctrl+V",
            vec!["key", "29:1", "47:1", "47:0", "29:0"],
        )
    };

    tracing::debug!("Clipboard: trying ydotool key for {}", combo);
    match std::process::Command::new("ydotool").args(&args).output() {
        Ok(output) if output.status.success() => {
            tracing::info!("Clipboard: {} sent via ydotool", combo);
            true
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                "ydotool key failed for {} (status {}): {}",
                combo,
                output.status,
                stderr.trim()
            );
            false
        }
        Err(e) => {
            tracing::debug!("ydotool unavailable: {}", e);
            false
        }
    }
}

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

/// Decide which paste keystroke to send. Precedence:
///   1. BEAMER_PASTE_SHORTCUT env var ("ctrl_v" | "ctrl_shift_v" | "auto")
///   2. injection.paste_shortcut in config.toml
///   3. Default: "auto" — queries the Beamer GNOME focus helper extension
///      (if installed and enabled) to pick per-app. Falls back to Ctrl+Shift+V
///      when the extension is absent or the call fails.
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

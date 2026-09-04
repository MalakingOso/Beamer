use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use arboard::Clipboard;

pub struct ClipboardBackend {
    /// Configured paste shortcut, threaded in at construction so pastes
    /// never re-read disk. Unused on Windows (always plain Ctrl+V).
    #[cfg(not(target_os = "windows"))]
    pub paste_shortcut: String,
}

impl InjectionBackend for ClipboardBackend {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    fn display_name(&self) -> &'static str {
        "Clipboard (Ctrl+V paste)"
    }

    fn available(&self) -> Result<(), String> {
        Clipboard::new().map(|_| ()).map_err(|e| format!("Clipboard unavailable: {}", e))
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        #[cfg(target_os = "windows")]
        let auto_pasted = inject_via_clipboard(text)?;
        #[cfg(not(target_os = "windows"))]
        let auto_pasted = inject_via_clipboard(text, &self.paste_shortcut)?;

        #[cfg(target_os = "windows")]
        let target_info = "via Ctrl+V paste".to_string();
        #[cfg(not(target_os = "windows"))]
        let target_info = match auto_pasted {
            Some(mechanism) => format!("pasted via {} chord", mechanism),
            None => "set on clipboard — paste manually".to_string(),
        };
        let _ = auto_pasted;

        Ok(InjectionResult {
            method: "Clipboard".into(),
            target_info,
        })
    }
}

/// Set clipboard text and paste with Ctrl+V; restores the previous clipboard after.
#[cfg(target_os = "windows")]
fn inject_via_clipboard(text: &str) -> Result<bool> {
    let mut clipboard = Clipboard::new()?;
    let saved = clipboard.get_text().ok();

    tracing::info!("Clipboard: setting {} bytes of text", text.len());
    if clipboard.set_text(text).is_err() {
        anyhow::bail!("Failed to set clipboard text");
    }

    std::thread::sleep(std::time::Duration::from_millis(80));
    tracing::info!("Clipboard: sending Ctrl+V");
    send_ctrl_v()?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Some(saved_text) = saved {
        let _ = clipboard.set_text(saved_text);
    }
    Ok(true)
}

/// Paste via the first working chord (GNOME helper → ydotool → wtype).
/// `None` means manual paste: text stays on the clipboard and a
/// notification says so (previous clipboard deliberately not restored).
#[cfg(not(target_os = "windows"))]
fn inject_via_clipboard(text: &str, paste_shortcut: &str) -> Result<Option<&'static str>> {
    let mut clipboard = Clipboard::new()?;
    let saved = clipboard.get_text().ok();

    tracing::info!("Clipboard: setting {} bytes of text", text.len());
    set_clipboard_linux(text, &mut clipboard)?;

    // Let the compositor advertise the new clipboard offer before pasting.
    std::thread::sleep(std::time::Duration::from_millis(150));

    if let Some(mechanism) = try_paste_chord(paste_shortcut) {
        std::thread::sleep(std::time::Duration::from_millis(500));
        // Restore only after the target has read the offer — too early pastes OLD content.
        if let Some(saved_text) = saved {
            let _ = clipboard.set_text(saved_text);
        }
        Ok(Some(mechanism))
    } else {
        tracing::info!(
            "Clipboard: no chord mechanism available — text left on clipboard for manual paste"
        );
        notify_manual_paste();
        let _ = clipboard; // keep handle alive until the selection is served
        Ok(None)
    }
}

/// Tell the user the transcript is waiting on the clipboard for manual paste.
#[cfg(not(target_os = "windows"))]
fn notify_manual_paste() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = notify_rust::Notification::new()
            .appname("Beamer")
            .summary("Beamer")
            .body("Copied to clipboard — press Ctrl+V to paste (Ctrl+Shift+V in terminals)")
            .show()
        {
            tracing::warn!("Manual-paste notification failed: {}", e);
        }
    }
}

/// Set clipboard on Linux, with wl-copy fallback and verification.
#[cfg(not(target_os = "windows"))]
fn set_clipboard_linux(text: &str, clipboard: &mut Clipboard) -> Result<()> {
    let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    let arboard_ok = clipboard.set_text(text).is_ok();
    if arboard_ok {
        tracing::debug!("Clipboard: arboard set_text succeeded");
    }

    if is_wayland {
        // On Wayland verify the set actually landed.
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

/// Run a command with a hard timeout. Needed because wl-paste can hang
/// indefinitely on some GNOME compositor states, mid-injection.
#[cfg(not(target_os = "windows"))]
fn run_with_timeout(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<Option<std::process::Output>> {
    use std::io::Read;

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr);
                }
                return Ok(Some(std::process::Output { status, stdout, stderr }));
            }
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// Verify the clipboard actually contains the expected text using wl-paste.
#[cfg(not(target_os = "windows"))]
fn verify_clipboard_contains(expected: &str) -> bool {
    let result = run_with_timeout(
        std::process::Command::new("wl-paste").arg("--no-newline"),
        std::time::Duration::from_millis(500),
    );
    match result {
        Ok(None) => {
            tracing::warn!("wl-paste timed out after 500ms — assuming clipboard was set");
            true
        }
        Ok(Some(out)) if out.status.success() => {
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
        Ok(Some(out)) => {
            tracing::warn!("wl-paste exited with status {}", out.status);
            true // Can't verify, assume it worked
        }
        Err(e) => {
            tracing::warn!("wl-paste not available: {}", e);
            true // Can't verify, assume it worked
        }
    }
}

/// Reap a daemonized `wl-copy` child (it forks to serve the selection;
/// the spawned process exits once waited on — never waiting leaks a zombie).
/// Polls briefly instead of `wait()` so a foregrounded wl-copy can't block injection.
#[cfg(not(target_os = "windows"))]
pub(crate) fn reap_daemonized(mut child: std::process::Child, what: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if std::time::Instant::now() >= deadline => {
                tracing::debug!("{} did not exit within 500ms — leaving it running", what);
                return;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => {
                tracing::debug!("Could not wait on {}: {}", what, e);
                return;
            }
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
    reap_daemonized(child, "wl-copy");
    tracing::debug!("Clipboard: wl-copy process spawned");
    Ok(())
}

#[cfg(target_os = "windows")]
fn send_ctrl_v() -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, VIRTUAL_KEY};

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

/// Paste chord via the first working mechanism: GNOME helper → ydotool → wtype.
/// Chord variant comes from `resolve_use_shift_v`. Returns the winner, or `None`.
#[cfg(not(target_os = "windows"))]
fn try_paste_chord(paste_shortcut: &str) -> Option<&'static str> {
    let use_shift = resolve_use_shift_v(paste_shortcut);
    if crate::injection::gnome::send_paste_chord(use_shift) {
        tracing::info!("Clipboard: paste chord sent via GNOME helper");
        return Some("GNOME helper");
    }
    if try_ydotool_paste(use_shift) {
        return Some("ydotool");
    }
    if crate::injection::wtype::send_paste_chord(use_shift) {
        tracing::info!("Clipboard: paste chord sent via wtype");
        return Some("wtype");
    }
    None
}

/// Paste chord via ydotool. Keycodes: 29 = Ctrl, 42 = Shift, 47 = V (`:1` down, `:0` up).
#[cfg(not(target_os = "windows"))]
fn try_ydotool_paste(use_shift: bool) -> bool {
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

/// Pure Ctrl+Shift+V decision from setting + focused app id. True = Shift+V.
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

/// Paste chord precedence: `BEAMER_PASTE_SHORTCUT` env > `configured`
/// (loaded once at session start; mid-session config edits need a restart).
/// "auto" picks per-app via the focus helper, defaulting to Ctrl+Shift+V.
#[cfg(not(target_os = "windows"))]
fn resolve_use_shift_v(configured: &str) -> bool {
    let setting = std::env::var("BEAMER_PASTE_SHORTCUT")
        .ok()
        .unwrap_or_else(|| {
            if configured.is_empty() {
                "auto".to_string()
            } else {
                configured.to_string()
            }
        });

    let focused = crate::injection::focus::focused_app_id();
    let use_shift = choose_use_shift_v(&setting, focused.as_deref());

    let combo = if use_shift { "Ctrl+Shift+V" } else { "Ctrl+V" };
    match focused.as_deref() {
        Some(app) => tracing::info!("Clipboard: paste_shortcut={} focus={} → {}", setting, app, combo),
        None => tracing::info!("Clipboard: paste_shortcut={} focus=unknown → {}", setting, combo),
    }
    use_shift
}

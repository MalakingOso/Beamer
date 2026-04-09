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
        inject_via_clipboard(text)?;
        Ok(InjectionResult {
            method: "Clipboard".into(),
            target_info: "via Ctrl+V paste".into(),
        })
    }
}

/// Sets clipboard text, verifies it was set, sends Ctrl+V, then restores clipboard.
fn inject_via_clipboard(text: &str) -> Result<()> {
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

    // --- Step 2: Send Ctrl+V ---
    std::thread::sleep(std::time::Duration::from_millis(80));

    tracing::info!("Clipboard: sending Ctrl+V");
    send_ctrl_v()?;

    // Give the target app time to process the paste
    std::thread::sleep(std::time::Duration::from_millis(500));

    // --- Step 3: Restore clipboard ---
    if let Some(saved_text) = saved {
        let _ = clipboard.set_text(saved_text);
    }

    Ok(())
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

#[cfg(not(target_os = "windows"))]
fn send_ctrl_v() -> Result<()> {
    // Try ydotool first — kernel-level uinput, works in ALL apps including browsers
    tracing::debug!("Clipboard: trying ydotool key for Ctrl+V");
    if let Ok(output) = std::process::Command::new("ydotool")
        .arg("key")
        .arg("29:1")  // Ctrl down
        .arg("47:1")  // V down
        .arg("47:0")  // V up
        .arg("29:0")  // Ctrl up
        .output()
    {
        if output.status.success() {
            tracing::info!("Clipboard: Ctrl+V sent via ydotool");
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("ydotool key failed (status {}): {}", output.status, stderr.trim());
    } else {
        tracing::debug!("ydotool not found, trying dotool");
    }

    // Try dotool — also kernel-level uinput
    tracing::debug!("Clipboard: trying dotool for Ctrl+V");
    if let Ok(mut child) = std::process::Command::new("dotool")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(b"key ctrl+v");
        }
        if let Ok(status) = child.wait() {
            if status.success() {
                tracing::info!("Clipboard: Ctrl+V sent via dotool");
                return Ok(());
            }
        }
    }

    // Try wtype on Wayland (uses virtual keyboard — may not work in all compositors)
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        tracing::debug!("Clipboard: trying wtype for Ctrl+V");
        if let Ok(output) = std::process::Command::new("wtype")
            .arg("-M")
            .arg("ctrl")
            .arg("v")
            .arg("-m")
            .arg("ctrl")
            .output()
        {
            if output.status.success() {
                tracing::info!("Clipboard: Ctrl+V sent via wtype");
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("wtype Ctrl+V failed: {}", stderr.trim());
        }
    }

    // Last resort: enigo
    tracing::debug!("Clipboard: trying enigo for Ctrl+V");
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Control, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Release)?;
    enigo.key(Key::Control, Direction::Release)?;
    tracing::info!("Clipboard: Ctrl+V sent via enigo");
    Ok(())
}

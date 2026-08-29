#![cfg(target_os = "windows")]

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY,
};

/// Apps whose custom input pipelines silently drop synthetic Unicode events,
/// making SendInput ineffective. These skip straight to the next backend.
///
/// Windows 11's redesigned Notepad has the same failure class, documented in
/// community reports (AutoHotkey forums) as a Microsoft-acknowledged
/// Notepad-redesign bug: batched synthetic Unicode keystrokes get buffered or
/// dropped, sometimes not committing until real user input arrives — and
/// SendInput's own return value can't see this, since every event is still
/// accepted into the input queue (`sent == inputs.len()` above). `notepad.exe`
/// is deliberately NOT added here yet: this app's own clipboard fallback
/// (`clipboard.rs::inject_via_clipboard`, Windows path) restores the previous
/// clipboard on a blind fixed 500ms timer with no confirmation the paste
/// actually landed, so routing Notepad to clipboard under a slow/laggy RDP
/// session risks trading a garbled-text bug for a stale-clipboard-content
/// bug. Fix that restore-timing race first, then add `notepad.exe` here.
const SKIP_SENDINPUT_PROCESSES: &[&str] = &["warp.exe"];

pub struct SendInputBackend;

impl InjectionBackend for SendInputBackend {
    fn name(&self) -> &'static str {
        "sendinput"
    }

    fn display_name(&self) -> &'static str {
        "SendInput (Unicode key events)"
    }

    fn available(&self) -> Result<(), String> {
        // Check if the foreground process is in the skip list
        if let Some(process) = get_foreground_process_name() {
            if SKIP_SENDINPUT_PROCESSES
                .iter()
                .any(|&p| process.eq_ignore_ascii_case(p))
            {
                return Err(format!("Skipped for process '{}'", process));
            }
        }
        Ok(())
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let success = inject_via_sendinput(text)?;
        if success {
            Ok(InjectionResult {
                method: "SendInput".into(),
                target_info: "via Unicode key events".into(),
            })
        } else {
            anyhow::bail!("SendInput failed")
        }
    }
}

/// Inject text by synthesizing Unicode keyboard events via `SendInput`.
/// Each UTF-16 code unit gets a key-down + key-up pair with `KEYEVENTF_UNICODE`,
/// which bypasses keyboard layout mapping and works for any Unicode character.
pub fn inject_via_sendinput(text: &str) -> Result<bool> {
    let mut inputs: Vec<INPUT> = Vec::new();

    for ch in text.encode_utf16() {
        inputs.push(make_unicode_input(ch, false));
        inputs.push(make_unicode_input(ch, true));
    }

    if inputs.is_empty() {
        return Ok(true);
    }

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };

    if sent as usize != inputs.len() {
        tracing::warn!("SendInput: sent {} of {} events", sent, inputs.len());
    }

    // A partial queue-insertion used to count as success (`sent > 0`), which
    // stopped the fallback chain from ever reaching clipboard/uia on the rest
    // of the text. Note this only catches SendInput's own queue-insertion
    // failures — it can't see the Windows 11 Notepad failure mode below,
    // where every event is accepted into the queue (sent == inputs.len()),
    // and the target's own input pipeline is what drops or reorders them.
    Ok(sent as usize == inputs.len())
}

fn make_unicode_input(char_code: u16, key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }

    INPUT {
        r#type: INPUT_TYPE(1), // INPUT_KEYBOARD
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: char_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn get_foreground_process_name() -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);

        if ok.is_err() {
            return None;
        }

        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().map(|s| s.to_string())
    }
}

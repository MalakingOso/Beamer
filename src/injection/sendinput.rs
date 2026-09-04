#![cfg(target_os = "windows")]

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY,
};

/// Processes whose input pipelines drop synthetic Unicode events, so
/// SendInput silently does nothing there. Undetectable via the SendInput
/// return value (events are still accepted into the queue) — skip to next backend.
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

/// Synthesize Unicode key events via `SendInput` (`KEYEVENTF_UNICODE`
/// bypasses layout mapping). Each UTF-16 unit gets a down + up pair.
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

    // Require full queue insertion so a partial send still falls through
    // to the next backend. (Can't catch targets that accept events but drop them.)
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

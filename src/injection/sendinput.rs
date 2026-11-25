#![cfg(target_os = "windows")]

//! Win32 `SendInput` backend, first in the Windows chain: types each UTF-16 unit
//! as a `KEYEVENTF_UNICODE` key event, layout-independent, inserting at the cursor.
//! Input into elevated windows is blocked by UIPI (dictation there does nothing);
//! see `agent_docs/text_injection.md`.

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY,
};

/// Processes that accept synthetic Unicode events into the queue and then drop
/// them, so SendInput silently types nothing. Undetectable from the return value,
/// hence skipped by name. `notepad.exe` is deliberately absent (verified working).
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

    let size = std::mem::size_of::<INPUT>() as i32;
    let mut offset = 0usize;
    let mut retries = 0u32;
    while offset < inputs.len() {
        let slice = &inputs[offset..];
        let sent = unsafe { SendInput(slice, size) } as usize;
        offset = (offset + sent).min(inputs.len());
        if offset == inputs.len() {
            return Ok(true);
        }
        // UIPI blocks all events (`sent == 0` on the very first call): fall
        // through immediately rather than sleeping on an elevated window.
        if offset == 0 {
            tracing::warn!("SendInput: blocked (sent 0 of {} events)", inputs.len());
            return Ok(false);
        }
        retries += 1;
        if retries > 5 {
            tracing::warn!(
                "SendInput: gave up after {} retries ({} of {} events sent)",
                retries - 1,
                offset,
                inputs.len()
            );
            return Ok(false);
        }
        tracing::warn!(
            "SendInput: sent {} of {} events so far; backing off before tail retry {}",
            offset,
            inputs.len(),
            retries
        );
        // Brief pause lets the target drain its input queue before retrying the tail.
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(true)
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

/// True when the foreground window belongs to this process (sticky notes,
/// settings, board). Dispatch-only, never in `available()`; see `super::foreground_is_self`.
pub(crate) fn foreground_is_self() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid != 0 && pid == GetCurrentProcessId()
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

pub mod clipboard;
pub mod sendinput;
pub mod uia;

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct InjectionResult {
    pub method: String,
    pub target_info: String,
}

/// Apps whose custom input pipelines silently drop synthetic Unicode events,
/// making SendInput ineffective. These skip straight to clipboard injection.
const SKIP_SENDINPUT_PROCESSES: &[&str] = &["warp.exe"];

/// Inject text into the focused window. Runs the entire Win32/COM fallback chain
/// on a blocking thread (required because UIA and SendInput are synchronous COM calls).
///
/// Fallback order: SendInput → Clipboard paste → UIA SetValue (last resort).
pub async fn inject_text(text: &str, preferred: &str) -> Result<InjectionResult> {
    let text = text.to_string();
    let preferred = preferred.to_string();

    tokio::task::spawn_blocking(move || inject_text_blocking(&text, &preferred))
        .await?
}

fn inject_text_blocking(text: &str, preferred: &str) -> Result<InjectionResult> {
    match preferred {
        "uia" => return try_uia(text),
        "sendinput" => return try_sendinput(text),
        "clipboard" => return try_clipboard(text),
        _ => {} // "auto" — run the full fallback chain below
    }

    let process_name = get_foreground_process_name().unwrap_or_default();
    let skip_sendinput = SKIP_SENDINPUT_PROCESSES
        .iter()
        .any(|&p| process_name.eq_ignore_ascii_case(p));

    if skip_sendinput {
        tracing::info!(
            "Skipping SendInput for process '{}' — using clipboard directly",
            process_name
        );
    }

    // 1. SendInput — types at cursor position, preserves existing text
    if !skip_sendinput {
        match try_sendinput(text) {
            Ok(result) => {
                tracing::info!("Injection succeeded via {}: {}", result.method, result.target_info);
                return Ok(result);
            }
            Err(e) => tracing::debug!("SendInput failed: {}", e),
        }
    }

    // 2. Clipboard Ctrl+V — pastes at cursor, preserves existing text
    match try_clipboard(text) {
        Ok(result) => {
            tracing::info!("Injection succeeded via {}: {}", result.method, result.target_info);
            return Ok(result);
        }
        Err(e) => tracing::debug!("Clipboard failed: {}", e),
    }

    // 3. UIA SetValue — last resort, replaces entire field value
    let result = try_uia(text)?;
    tracing::info!("Injection succeeded via {}: {}", result.method, result.target_info);
    Ok(result)
}

/// Get the executable name (e.g. "warp.exe") of the foreground window's process.
/// Used to apply per-app injection workarounds via `SKIP_SENDINPUT_PROCESSES`.
fn get_foreground_process_name() -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
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

fn try_uia(text: &str) -> Result<InjectionResult> {
    let result = uia::try_inject_set_value(text)?;
    if result.success {
        Ok(InjectionResult {
            method: result.method.to_string(),
            target_info: result.target_info,
        })
    } else {
        anyhow::bail!("UIA SetValue failed for: {}", result.target_info)
    }
}

fn try_sendinput(text: &str) -> Result<InjectionResult> {
    let success = sendinput::inject_via_sendinput(text)?;
    if success {
        Ok(InjectionResult {
            method: "SendInput".to_string(),
            target_info: "via Unicode key events".to_string(),
        })
    } else {
        anyhow::bail!("SendInput failed")
    }
}

fn try_clipboard(text: &str) -> Result<InjectionResult> {
    let success = clipboard::inject_via_clipboard(text)?;
    if success {
        Ok(InjectionResult {
            method: "Clipboard".to_string(),
            target_info: "via Ctrl+V paste".to_string(),
        })
    } else {
        anyhow::bail!("Clipboard injection failed")
    }
}

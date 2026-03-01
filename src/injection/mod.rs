pub mod clipboard;
pub mod sendinput;
pub mod uia;

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct InjectionResult {
    pub method: String,
    pub target_info: String,
}

/// Inject text using the fallback chain: UIA SetValue → SendInput → Clipboard
/// All Win32/COM calls run on a blocking thread.
pub async fn inject_text(text: &str, preferred: &str) -> Result<InjectionResult> {
    let text = text.to_string();
    let preferred = preferred.to_string();

    tokio::task::spawn_blocking(move || inject_text_blocking(&text, &preferred))
        .await?
}

fn inject_text_blocking(text: &str, preferred: &str) -> Result<InjectionResult> {
    // If a specific method is preferred, try it first
    match preferred {
        "uia" => return try_uia(text),
        "sendinput" => return try_sendinput(text),
        "clipboard" => return try_clipboard(text),
        _ => {} // "auto" — use fallback chain
    }

    // Fallback chain
    // 1. Try UIA SetValue
    match try_uia(text) {
        Ok(result) => {
            tracing::info!("Injection succeeded via {}: {}", result.method, result.target_info);
            return Ok(result);
        }
        Err(e) => tracing::debug!("UIA failed: {}", e),
    }

    // 2. Try SendInput
    match try_sendinput(text) {
        Ok(result) => {
            tracing::info!("Injection succeeded via {}: {}", result.method, result.target_info);
            return Ok(result);
        }
        Err(e) => tracing::debug!("SendInput failed: {}", e),
    }

    // 3. Clipboard fallback
    let result = try_clipboard(text)?;
    tracing::info!("Injection succeeded via {}: {}", result.method, result.target_info);
    Ok(result)
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

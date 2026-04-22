pub mod clipboard;

#[cfg(target_os = "windows")]
pub mod sendinput;
#[cfg(target_os = "windows")]
pub mod uia;

#[cfg(not(target_os = "windows"))]
pub mod ydotool;

use anyhow::Result;

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InjectionResult {
    pub method: String,
    pub target_info: String,
}

/// A text injection backend. Each implementation is self-contained
/// with its own availability checks and injection logic.
pub trait InjectionBackend: Send + Sync {
    /// Machine-readable name matching the TOML config value.
    fn name(&self) -> &'static str;
    /// Human-readable label for the Settings UI.
    fn display_name(&self) -> &'static str;
    /// Check whether this backend can run right now.
    fn available(&self) -> Result<(), String>;
    /// Inject the given text into the currently focused window.
    fn inject(&self, text: &str) -> Result<InjectionResult>;
}

// ─── Registry ─────────────────────────────────────────────────────────────────

/// All backends available on this platform.
pub fn all_backends() -> Vec<Box<dyn InjectionBackend>> {
    let mut backends: Vec<Box<dyn InjectionBackend>> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        backends.push(Box::new(sendinput::SendInputBackend));
        backends.push(Box::new(clipboard::ClipboardBackend));
        backends.push(Box::new(uia::UiaBackend));
    }

    #[cfg(not(target_os = "windows"))]
    {
        backends.push(Box::new(ydotool::YdotoolBackend));
        backends.push(Box::new(clipboard::ClipboardBackend));
    }

    backends
}

/// Check which backends are available (for Settings UI display).
/// Returns (name, display_name, availability_result) for each backend.
pub fn check_availability() -> Vec<(&'static str, &'static str, Result<(), String>)> {
    all_backends()
        .iter()
        .map(|b| (b.name(), b.display_name(), b.available()))
        .collect()
}

/// Platform default backend order.
pub fn default_backend_names() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec!["sendinput".into(), "clipboard".into(), "uia".into()]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec!["ydotool".into(), "clipboard".into()]
    }
}

// ─── Dispatch ─────────────────────────────────────────────────────────────────

/// Inject text into the focused window using the configured backend chain.
pub async fn inject_text(text: &str, backends: &[String]) -> Result<InjectionResult> {
    let text = text.to_string();
    let backends = backends.to_vec();

    tokio::task::spawn_blocking(move || inject_text_blocking(&text, &backends)).await?
}

fn inject_text_blocking(text: &str, backend_names: &[String]) -> Result<InjectionResult> {
    let all = all_backends();
    let mut errors = Vec::new();

    for name in backend_names {
        if let Some(backend) = all.iter().find(|b| b.name() == name.as_str()) {
            match backend.available() {
                Ok(()) => match backend.inject(text) {
                    Ok(result) => {
                        tracing::info!(
                            "Injection succeeded via {}: {}",
                            result.method,
                            result.target_info
                        );
                        return Ok(result);
                    }
                    Err(e) => {
                        tracing::info!("{} injection failed → falling through: {:#}", name, e);
                        errors.push(format!("{}: {}", name, e));
                    }
                },
                Err(reason) => {
                    tracing::info!("{} unavailable → falling through: {}", name, reason);
                }
            }
        } else {
            tracing::debug!("Unknown backend '{}', skipping", name);
        }
    }

    if errors.is_empty() {
        anyhow::bail!("No injection backends were available")
    } else {
        anyhow::bail!("All injection backends failed: {}", errors.join("; "))
    }
}

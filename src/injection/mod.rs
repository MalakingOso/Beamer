pub mod clipboard;

#[cfg(target_os = "windows")]
pub mod sendinput;
#[cfg(target_os = "windows")]
pub mod uia;

#[cfg(not(target_os = "windows"))]
pub mod ydotool;
#[cfg(not(target_os = "windows"))]
pub mod focus;
#[cfg(not(target_os = "windows"))]
pub mod gnome;
#[cfg(not(target_os = "windows"))]
pub mod wtype;

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

// ─── Text sanitation ──────────────────────────────────────────────────────────

/// Prepare text for keystroke-based injection (gnome/wtype/ydotool backends).
/// Transliterates the smart punctuation transcription APIs produce, and maps
/// newlines/tabs to spaces — a typed Enter would submit chat boxes and forms
/// mid-injection. The clipboard path keeps newlines: an atomic paste doesn't
/// press keys.
#[cfg(not(target_os = "windows"))]
pub(crate) fn sanitize_for_typing(text: &str) -> String {
    let transliterated = text
        .replace('\u{2018}', "'") // left single quote
        .replace('\u{2019}', "'") // right single quote
        .replace('\u{201C}', "\"") // left double quote
        .replace('\u{201D}', "\"") // right double quote
        .replace('\u{2013}', "-") // en dash
        .replace('\u{2014}', "--") // em dash
        .replace('\u{2026}', "...") // ellipsis
        .replace('\u{00A0}', " "); // non-breaking space

    transliterated
        .replace("\r\n", " ")
        .replace(['\r', '\n', '\t'], " ")
        .trim_end()
        .to_string()
}

#[cfg(all(test, not(target_os = "windows")))]
mod sanitize_tests {
    use super::sanitize_for_typing;

    #[test]
    fn smart_punctuation_becomes_ascii() {
        assert_eq!(
            sanitize_for_typing("\u{2018}a\u{2019} \u{201C}b\u{201D} c\u{2013}d\u{2014}e\u{2026}"),
            "'a' \"b\" c-d--e..."
        );
    }

    #[test]
    fn newlines_become_spaces_never_enter() {
        assert_eq!(sanitize_for_typing("one\ntwo"), "one two");
        assert_eq!(sanitize_for_typing("one\ttwo"), "one two");
    }

    #[test]
    fn crlf_is_one_space() {
        assert_eq!(sanitize_for_typing("one\r\ntwo"), "one two");
    }

    #[test]
    fn trailing_newline_trimmed_interior_spacing_kept() {
        assert_eq!(sanitize_for_typing("one  two\n"), "one  two");
    }

    #[test]
    fn unicode_passes_through() {
        assert_eq!(sanitize_for_typing("naïve café 你好"), "naïve café 你好");
    }

    #[test]
    fn mixed_smart_quotes_and_em_dash_adjacency() {
        // Quote and dash transliterations abut with no separating text —
        // pins that each replace() only touches its own code point and
        // doesn't get confused by neighboring ASCII output from another.
        assert_eq!(sanitize_for_typing("\u{201C}\u{2014}\u{201D}"), "\"--\"");
        assert_eq!(
            sanitize_for_typing("\u{2018}\u{2014}\u{2019}word"),
            "'--'word"
        );
    }

    #[test]
    fn crlf_collapses_to_exactly_one_space_not_two() {
        // "\r\n" is consumed whole by the first replace("\r\n", " ") pass;
        // if it were double-processed by the later per-char replace of
        // ['\r','\n','\t'] each crlf would produce two spaces instead of one.
        assert_eq!(sanitize_for_typing("a\r\nb"), "a b");
        assert_eq!(sanitize_for_typing("a\r\nb\r\nc"), "a b c");
        // A bare "\r\n" becomes a single space, then trim_end() removes it.
        assert_eq!(sanitize_for_typing("\r\n"), "");
    }

    #[test]
    fn lone_cr_without_lf_becomes_space() {
        assert_eq!(sanitize_for_typing("one\rtwo"), "one two");
        assert_eq!(sanitize_for_typing("\r"), "");
    }

    #[test]
    fn non_bmp_emoji_passes_through() {
        assert_eq!(
            sanitize_for_typing("hi \u{1F600} there"),
            "hi \u{1F600} there"
        );
    }

    #[test]
    fn ellipsis_char_becomes_three_dots() {
        assert_eq!(sanitize_for_typing("Wait\u{2026}"), "Wait...");
    }

    #[test]
    fn trailing_whitespace_only_input_becomes_empty() {
        assert_eq!(sanitize_for_typing("   "), "");
        assert_eq!(sanitize_for_typing("text \n  "), "text");
    }
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
        backends.push(Box::new(gnome::GnomeBackend));
        backends.push(Box::new(wtype::WtypeBackend));
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
        vec![
            "gnome".into(),
            "wtype".into(),
            "ydotool".into(),
            "clipboard".into(),
        ]
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

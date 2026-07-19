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
    let mut out = String::with_capacity(text.len());
    // Set when the previous char emitted was a '\r' mapping, so a following
    // '\n' (i.e. a "\r\n" pair) contributes no extra space of its own.
    let mut last_was_cr = false;

    for ch in text.chars() {
        if last_was_cr {
            last_was_cr = false;
            if ch == '\n' {
                continue;
            }
        }

        match ch {
            '\u{2018}' | '\u{2019}' => out.push('\''), // smart single quotes
            '\u{201C}' | '\u{201D}' => out.push('"'),  // smart double quotes
            '\u{2013}' => out.push('-'),               // en dash
            '\u{2014}' => out.push_str("--"),          // em dash
            '\u{2026}' => out.push_str("..."),         // ellipsis
            '\u{00A0}' => out.push(' '),               // non-breaking space
            '\r' => {
                out.push(' ');
                last_was_cr = true;
            }
            '\n' | '\t' => out.push(' '),
            other => out.push(other),
        }
    }

    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    out
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
        // The '\r' arm pushes a space and sets `last_was_cr`; the following
        // '\n' is then swallowed by the `last_was_cr` check at the top of
        // the loop instead of falling through to its own '\n' => push(' ')
        // arm, so a "\r\n" pair produces exactly one space, not two.
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

/// All backends available on this platform. `paste_shortcut` is the
/// already-loaded `injection.paste_shortcut` config value, threaded into the
/// clipboard backend so it never re-reads config.toml per paste.
pub fn all_backends(paste_shortcut: &str) -> Vec<Box<dyn InjectionBackend>> {
    let mut backends: Vec<Box<dyn InjectionBackend>> = Vec::new();
    // Referenced unconditionally so the parameter isn't "unused" on the
    // platform whose branch below doesn't read it.
    let _ = paste_shortcut;

    #[cfg(target_os = "windows")]
    {
        backends.push(Box::new(sendinput::SendInputBackend));
        backends.push(Box::new(clipboard::ClipboardBackend {}));
        backends.push(Box::new(uia::UiaBackend));
    }

    #[cfg(not(target_os = "windows"))]
    {
        backends.push(Box::new(gnome::GnomeBackend));
        backends.push(Box::new(wtype::WtypeBackend));
        backends.push(Box::new(ydotool::YdotoolBackend));
        backends.push(Box::new(clipboard::ClipboardBackend {
            paste_shortcut: paste_shortcut.to_string(),
        }));
    }

    backends
}

/// Check which backends are available (for Settings UI display). Doesn't
/// invoke `inject()`, so the paste shortcut value passed here is irrelevant —
/// "auto" is a harmless placeholder.
pub fn check_availability() -> Vec<(&'static str, &'static str, Result<(), String>)> {
    all_backends("auto")
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
/// `paste_shortcut` is the caller's already-loaded `injection.paste_shortcut`
/// config value (passed down instead of reloading config.toml per call).
pub async fn inject_text(text: &str, backends: &[String], paste_shortcut: &str) -> Result<InjectionResult> {
    let text = text.to_string();
    let backends = backends.to_vec();
    let paste_shortcut = paste_shortcut.to_string();

    tokio::task::spawn_blocking(move || inject_text_blocking(&text, &backends, &paste_shortcut)).await?
}

fn inject_text_blocking(text: &str, backend_names: &[String], paste_shortcut: &str) -> Result<InjectionResult> {
    let all = all_backends(paste_shortcut);
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

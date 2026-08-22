//! Embedded webfonts for secondary windows.
//!
//! The main window loads `assets/styles.css` through a `<link>`, so its
//! `@font-face` rules resolve against Dioxus's asset handler. Secondary windows
//! (stickies, pill, splash) inject their CSS as an inline `<style>` via
//! `with_custom_head` instead, which carries no `@font-face` at all — so every
//! `font-family:"DM Mono"` in those windows has silently fallen back to a
//! system font while the main window rendered in the real one.
//!
//! These rules close that gap with `data:` URIs. A URI cannot fail to resolve,
//! which an asset URL in a hand-built head string might: there is no visual
//! test here, and a font that silently falls back looks like a design choice
//! rather than a bug. The three faces total ~85 KB, encoded once per process.

use std::sync::OnceLock;

use base64::Engine;

const DM_MONO_REGULAR: &[u8] = include_bytes!("../../assets/fonts/DMMono-Regular.woff2");
const DM_MONO_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/DMMono-Medium.woff2");
const RECURSIVE_VARIABLE: &[u8] = include_bytes!("../../assets/fonts/Recursive-Variable.woff2");

fn face(family: &str, weight: &str, bytes: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!(
        "@font-face{{font-family:\"{family}\";font-weight:{weight};font-style:normal;\
         font-display:block;src:url(data:font/woff2;base64,{b64}) format(\"woff2\");}}"
    )
}

/// `@font-face` rules for the app's three faces, ready to concatenate into a
/// `with_custom_head` stylesheet. Built once and cached — a note window can be
/// opened many times per session and the encoding is pure.
///
/// `font-display:block` rather than `swap`: the bytes are already in memory, so
/// there is no network to wait on, and `swap` would flash a fallback face for a
/// frame on a window that is only ~320px wide.
pub fn embedded_font_css() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        format!(
            "{}{}{}",
            face("DM Mono", "400", DM_MONO_REGULAR),
            face("DM Mono", "500", DM_MONO_MEDIUM),
            // Recursive is a variable font: one file serves the whole range.
            face("Recursive", "300 1000", RECURSIVE_VARIABLE),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_the_app_uses_is_embedded() {
        let css = embedded_font_css();
        assert_eq!(css.matches("@font-face").count(), 3);
        assert!(css.contains("font-family:\"DM Mono\";font-weight:400"));
        assert!(css.contains("font-family:\"DM Mono\";font-weight:500"));
        assert!(css.contains("font-family:\"Recursive\";font-weight:300 1000"));
    }

    /// A `data:` URI is only useful if it decodes back to the file. Guards
    /// against a truncated `include_bytes!` or an encoder swap that emits
    /// URL-safe base64, which WebKit would reject.
    #[test]
    fn the_data_uri_round_trips_to_the_original_woff2() {
        let css = embedded_font_css();
        let marker = "base64,";
        let start = css.find(marker).expect("a data URI") + marker.len();
        let end = start + css[start..].find(')').expect("uri terminator");

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&css[start..end])
            .expect("must be standard base64, not URL-safe");

        assert_eq!(decoded, DM_MONO_REGULAR, "first face is DM Mono 400");
        // woff2 files begin with the signature "wOF2".
        assert_eq!(&decoded[..4], b"wOF2", "not a woff2 payload");
    }

    #[test]
    fn the_css_is_built_once_and_reused() {
        assert!(
            std::ptr::eq(embedded_font_css(), embedded_font_css()),
            "each call re-encoding ~85 KB would show up on every note open"
        );
    }
}

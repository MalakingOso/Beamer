//! Embedded webfonts as `data:` URI `@font-face` rules, prepended to the inline
//! `<style>` of the main window and sticky-note windows. Without them every
//! `font-family:"DM Mono"` silently falls back to a system font. Three faces,
//! ~85 KB, encoded once.

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

/// `@font-face` rules for the three faces, built once and cached. `block`, not
/// `swap`: bytes are in memory, so `swap` would only flash a fallback face.
pub fn embedded_font_css() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        format!(
            "{}{}{}",
            face("DM Mono", "400", DM_MONO_REGULAR),
            face("DM Mono", "500", DM_MONO_MEDIUM),
            face("Recursive", "300 1000", RECURSIVE_VARIABLE),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards a truncated `include_bytes!` or a URL-safe-base64 encoder swap.
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
        assert_eq!(&decoded[..4], b"wOF2", "not a woff2 payload");
    }
}

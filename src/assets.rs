use base64::Engine;
use std::sync::OnceLock;

/// Single consolidated icon PNG bytes, used by tray, window, and Linux desktop integration.
pub static ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// Cached base64-encoded icon for splash screen data URI (encoded once on first use).
static ICON_PNG_B64: OnceLock<String> = OnceLock::new();

/// Get the icon as a base64-encoded data URL for embedding in HTML/CSS.
pub fn icon_png_data_url() -> &'static str {
    ICON_PNG_B64.get_or_init(|| {
        let b64 = base64::engine::general_purpose::STANDARD.encode(ICON_PNG);
        format!("data:image/png;base64,{}", b64)
    })
}

/// DM Mono, the pill's typeface. Same files `assets/styles.css` `@font-face`s
/// for the main window, embedded here because the pill window is built from an
/// inline `custom_head` string and has no stylesheet to resolve relative URLs
/// against.
#[cfg(not(target_os = "linux"))]
static DM_MONO_REGULAR_WOFF2: &[u8] = include_bytes!("../assets/fonts/DMMono-Regular.woff2");
#[cfg(not(target_os = "linux"))]
static DM_MONO_MEDIUM_WOFF2: &[u8] = include_bytes!("../assets/fonts/DMMono-Medium.woff2");

#[cfg(not(target_os = "linux"))]
static DM_MONO_FACE_CSS: OnceLock<String> = OnceLock::new();

/// `@font-face` rules for DM Mono with the font data inlined as `data:` URIs.
///
/// The pill window used to pull DM Mono from `fonts.googleapis.com`, which
/// meant a local dictation app made an outbound request to Google every time
/// the pill window was created, and fell back to a system monospace whenever
/// the machine was offline — despite the very same font already shipping in
/// `assets/fonts/`. Built once and cached; the encoded string is ~40 KB.
#[cfg(not(target_os = "linux"))]
pub fn dm_mono_face_css() -> &'static str {
    DM_MONO_FACE_CSS.get_or_init(|| {
        let engine = base64::engine::general_purpose::STANDARD;
        format!(
            "@font-face{{font-family:\"DM Mono\";font-weight:400;font-style:normal;font-display:swap;\
             src:url(data:font/woff2;base64,{}) format(\"woff2\");}}\
             @font-face{{font-family:\"DM Mono\";font-weight:500;font-style:normal;font-display:swap;\
             src:url(data:font/woff2;base64,{}) format(\"woff2\");}}",
            engine.encode(DM_MONO_REGULAR_WOFF2),
            engine.encode(DM_MONO_MEDIUM_WOFF2),
        )
    })
}

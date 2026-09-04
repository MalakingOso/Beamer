use base64::Engine;
use std::sync::OnceLock;

/// App icon PNG, shared by tray, window, and Linux desktop integration.
pub static ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

static ICON_PNG_B64: OnceLock<String> = OnceLock::new();

/// Icon as a base64 data URL for embedding in HTML/CSS.
pub fn icon_png_data_url() -> &'static str {
    ICON_PNG_B64.get_or_init(|| {
        let b64 = base64::engine::general_purpose::STANDARD.encode(ICON_PNG);
        format!("data:image/png;base64,{}", b64)
    })
}

/// DM Mono for the pill window, which is built from an inline `custom_head`
/// string with no stylesheet to resolve font URLs against.
#[cfg(not(target_os = "linux"))]
static DM_MONO_REGULAR_WOFF2: &[u8] = include_bytes!("../assets/fonts/DMMono-Regular.woff2");
#[cfg(not(target_os = "linux"))]
static DM_MONO_MEDIUM_WOFF2: &[u8] = include_bytes!("../assets/fonts/DMMono-Medium.woff2");

#[cfg(not(target_os = "linux"))]
static DM_MONO_FACE_CSS: OnceLock<String> = OnceLock::new();

/// `@font-face` rules for DM Mono with inlined `data:` URIs (built once; ~40 KB).
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

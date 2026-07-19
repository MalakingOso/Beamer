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

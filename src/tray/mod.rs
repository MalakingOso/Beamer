use muda::{Menu, MenuItem, PredefinedMenuItem};
use std::sync::OnceLock;
use tray_icon::Icon;

static IDLE_ICON_RGBA: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();

/// Linux-only; read by `load_recording_icon` for the recording indicator.
#[cfg(target_os = "linux")]
static RECORDING_ICON_RGBA: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();

/// Menu item handles for matching tray menu events.
#[derive(Clone)]
pub struct TrayMenuItems {
    pub home: MenuItem,
    pub history: MenuItem,
    pub vocab: MenuItem,
    pub settings: MenuItem,
    pub paste_last: MenuItem,
    pub check_updates: MenuItem,
    pub quit: MenuItem,
}

pub fn build_tray_menu() -> (Menu, TrayMenuItems) {
    let menu = Menu::new();

    let home = MenuItem::new("Home", true, None);
    let history = MenuItem::new("History", true, None);
    let vocab = MenuItem::new("Vocab", true, None);
    let settings = MenuItem::new("Settings", true, None);
    let paste_last = MenuItem::new("Paste Last Transcript", true, None);
    let check_updates = MenuItem::new("Check for Updates", true, None);
    let quit = MenuItem::new("Quit", true, None);

    menu.append_items(&[
        &home,
        &history,
        &vocab,
        &settings,
        &PredefinedMenuItem::separator(),
        &paste_last,
        &check_updates,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .expect("Failed to build tray menu");

    let items = TrayMenuItems {
        home,
        history,
        vocab,
        settings,
        paste_last,
        check_updates,
        quit,
    };
    (menu, items)
}

/// A 1x1 transparent icon, so a corrupt or missing baked-in asset degrades
/// the tray instead of panicking the process at startup.
fn fallback_icon() -> Icon {
    Icon::from_rgba(vec![0, 0, 0, 0], 1, 1).expect("1x1 transparent icon is always valid")
}

fn decode_icon(bytes: &[u8], what: &str) -> (Vec<u8>, u32, u32) {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            (rgba.into_raw(), w, h)
        }
        Err(e) => {
            tracing::error!("Tray: failed to decode {}: {}", what, e);
            (vec![0, 0, 0, 0], 1, 1)
        }
    }
}

pub fn load_icon() -> Icon {
    let (rgba_data, width, height) = IDLE_ICON_RGBA
        .get_or_init(|| decode_icon(crate::assets::ICON_PNG, "tray icon"))
        .clone();
    Icon::from_rgba(rgba_data, width, height).unwrap_or_else(|_| fallback_icon())
}

/// Linux-only recording indicator icon.
#[cfg(target_os = "linux")]
pub fn load_recording_icon() -> Icon {
    let (rgba_data, width, height) = RECORDING_ICON_RGBA
        .get_or_init(|| {
            decode_icon(
                include_bytes!("../../assets/icon_recording.ico"),
                "recording icon",
            )
        })
        .clone();
    Icon::from_rgba(rgba_data, width, height).unwrap_or_else(|_| fallback_icon())
}

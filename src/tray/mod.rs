use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::Icon;
use std::sync::OnceLock;

/// Cached RGBA data for idle icon (decoded once on first use)
static IDLE_ICON_RGBA: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();

/// Cached RGBA data for recording icon (decoded once on first use)
///
/// Linux-only, not dead: its sole reader is `load_recording_icon` below,
/// which is itself only called from `ui::linux_integration`
/// (`#![cfg(target_os = "linux")]`) to swap the tray icon while recording.
#[cfg(target_os = "linux")]
static RECORDING_ICON_RGBA: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();

/// Menu item IDs used to match events in the tray menu handler.
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

pub fn load_icon() -> Icon {
    let (rgba_data, width, height) = IDLE_ICON_RGBA
        .get_or_init(|| {
            let icon_bytes = crate::assets::ICON_PNG;
            let img = image::load_from_memory(icon_bytes).expect("Failed to load icon");
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            (rgba.into_raw(), w, h)
        })
        .clone();
    Icon::from_rgba(rgba_data, width, height).expect("Failed to create tray icon")
}

/// Linux-only, not dead: called only from `ui::linux_integration`
/// (`#![cfg(target_os = "linux")]`), which swaps the tray icon while
/// recording. Gated here to match, rather than left to show up as unused on
/// every other target.
#[cfg(target_os = "linux")]
pub fn load_recording_icon() -> Icon {
    let (rgba_data, width, height) = RECORDING_ICON_RGBA
        .get_or_init(|| {
            let icon_bytes = include_bytes!("../../assets/icon_recording.ico");
            let img = image::load_from_memory(icon_bytes).expect("Failed to load recording icon");
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            (rgba.into_raw(), w, h)
        })
        .clone();
    Icon::from_rgba(rgba_data, width, height).expect("Failed to create tray icon")
}

use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::Icon;

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
    let icon_bytes = include_bytes!("../../assets/icon.png");
    let img = image::load_from_memory(icon_bytes).expect("Failed to load icon");
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).expect("Failed to create tray icon")
}

pub fn load_recording_icon() -> Icon {
    let icon_bytes = include_bytes!("../../assets/icon_recording.ico");
    let img = image::load_from_memory(icon_bytes).expect("Failed to load recording icon");
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).expect("Failed to create tray icon")
}

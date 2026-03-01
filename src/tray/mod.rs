use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::Icon;

#[derive(Clone)]
pub struct TrayMenuItems {
    pub settings: MenuItem,
    pub quit: MenuItem,
}

pub fn build_tray_menu() -> (Menu, TrayMenuItems) {
    let menu = Menu::new();
    let settings = MenuItem::new("Settings", true, None);
    let quit = MenuItem::new("Quit", true, None);

    menu.append_items(&[
        &settings,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .expect("Failed to build tray menu");

    let items = TrayMenuItems { settings, quit };
    (menu, items)
}

pub fn load_icon() -> Icon {
    let icon_bytes = include_bytes!("../../assets/icon.png");
    let img = image::load_from_memory(icon_bytes).expect("Failed to load icon");
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).expect("Failed to create tray icon")
}

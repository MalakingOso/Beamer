//! Tray menu and icon images. Not on the dictation path: `ui::app_setup`
//! registers the tray, `ui::app_menu` handles clicks, and on Linux
//! `ui::linux_integration` swaps in the recording icon while recording.

pub mod glyphs;
pub mod labels;

use glyphs::{icon as glyph_icon, Glyph};
use muda::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem};
use std::sync::OnceLock;
use tray_icon::Icon;

static IDLE_ICON_RGBA: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();

/// Linux-only; read by `load_recording_icon` for the recording indicator.
#[cfg(target_os = "linux")]
static RECORDING_ICON_RGBA: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();

/// Menu item handles for matching tray menu events and updating live labels.
#[derive(Clone)]
pub struct TrayMenuItems {
    /// Disabled header lines: recording state, and how to start dictating.
    pub status: MenuItem,
    pub hint: MenuItem,
    pub home: IconMenuItem,
    pub history: IconMenuItem,
    pub vocab: IconMenuItem,
    pub settings: IconMenuItem,
    pub hide_show_notes: IconMenuItem,
    pub paste_last: IconMenuItem,
    pub check_updates: IconMenuItem,
    pub quit: IconMenuItem,
}

fn entry(text: &str, enabled: bool, glyph: Glyph) -> IconMenuItem {
    IconMenuItem::new(text, enabled, glyph_icon(glyph), None)
}

pub fn build_tray_menu() -> (Menu, TrayMenuItems) {
    let menu = Menu::new();

    let status = MenuItem::new(labels::status_text(Default::default()), false, None);
    let hint = MenuItem::new("Dictation hotkey", false, None);
    let home = entry("Home", true, Glyph::Home);
    let history = entry("History", true, Glyph::History);
    let vocab = entry("Vocab", true, Glyph::Vocab);
    let settings = entry("Settings", true, Glyph::Settings);
    let (notes_text, notes_glyph) = labels::notes_item(0);
    let hide_show_notes = entry(&notes_text, true, notes_glyph);
    // Disabled until the first transcript exists (see `ui::app_menu`).
    let paste_last = entry("Paste Last Transcript", false, Glyph::Paste);
    let check_updates = entry("Check for Updates", true, Glyph::Update);
    let quit = entry("Quit", true, Glyph::Quit);

    if let Err(e) = menu.append_items(&[
        &status,
        &hint,
        &PredefinedMenuItem::separator(),
        &home,
        &history,
        &vocab,
        &settings,
        &PredefinedMenuItem::separator(),
        &hide_show_notes,
        &paste_last,
        &check_updates,
        &PredefinedMenuItem::separator(),
        &quit,
    ]) {
        tracing::error!("Failed to populate tray menu: {}", e);
    }

    let items = TrayMenuItems {
        status,
        hint,
        home,
        history,
        vocab,
        settings,
        hide_show_notes,
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

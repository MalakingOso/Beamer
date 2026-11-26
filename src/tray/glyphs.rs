//! Tray menu glyphs: Phosphor "regular" icons (the set `ui::icons` uses),
//! pre-rendered to 32px violet PNGs in `assets/tray/` and compiled in. Violet,
//! not Beamer's #4B0082 accent: a native menu is light or dark with the OS
//! theme and the glyph has to read on both.

use image::imageops::FilterType;
use muda::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Home,
    History,
    Vocab,
    Settings,
    NotesHide,
    NotesShow,
    Paste,
    Update,
    UpdateReady,
    Quit,
}

impl Glyph {
    fn png(self) -> &'static [u8] {
        match self {
            Glyph::Home => include_bytes!("../../assets/tray/home.png"),
            Glyph::History => include_bytes!("../../assets/tray/history.png"),
            Glyph::Vocab => include_bytes!("../../assets/tray/vocab.png"),
            Glyph::Settings => include_bytes!("../../assets/tray/settings.png"),
            Glyph::NotesHide => include_bytes!("../../assets/tray/notes-hide.png"),
            Glyph::NotesShow => include_bytes!("../../assets/tray/notes-show.png"),
            Glyph::Paste => include_bytes!("../../assets/tray/paste.png"),
            Glyph::Update => include_bytes!("../../assets/tray/update.png"),
            Glyph::UpdateReady => include_bytes!("../../assets/tray/update-ready.png"),
            Glyph::Quit => include_bytes!("../../assets/tray/quit.png"),
        }
    }
}

/// Side length of a menu icon. muda's Windows backend draws every menu icon
/// into a fixed 16x16 bitmap (`Icon::to_hbitmap`), so any other size is just
/// rescaled by GDI; hand it exactly 16 and do the downscale here, with a
/// proper filter, instead.
const MENU_ICON_PX: u32 = 16;

/// The glyph as a menu icon, or `None` (a text-only item) if the embedded
/// asset fails to decode: a bad PNG must not take the tray down.
pub fn icon(glyph: Glyph) -> Option<Icon> {
    let rgba = match image::load_from_memory(glyph.png()) {
        Ok(img) => img.to_rgba8(),
        Err(e) => {
            tracing::error!("Tray: failed to decode {:?} glyph: {}", glyph, e);
            return None;
        }
    };
    let px = MENU_ICON_PX;
    let rgba = image::imageops::resize(&rgba, px, px, FilterType::Lanczos3);
    Icon::from_rgba(rgba.into_raw(), px, px).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Glyph; 10] = [
        Glyph::Home,
        Glyph::History,
        Glyph::Vocab,
        Glyph::Settings,
        Glyph::NotesHide,
        Glyph::NotesShow,
        Glyph::Paste,
        Glyph::Update,
        Glyph::UpdateReady,
        Glyph::Quit,
    ];

    #[test]
    fn every_glyph_decodes_to_a_square_icon() {
        for glyph in ALL {
            let img = image::load_from_memory(glyph.png())
                .unwrap_or_else(|e| panic!("{glyph:?} does not decode: {e}"));
            assert_eq!((img.width(), img.height()), (32, 32), "{glyph:?}");
            assert!(icon(glyph).is_some(), "{glyph:?} should build a menu icon");
        }
    }
}

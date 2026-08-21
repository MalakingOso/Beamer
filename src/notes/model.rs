//! The shape of a note.
//!
//! Split out of `mod.rs` purely for the 500-line limit: the store grew search,
//! archive and restore and pushed the file over. Nothing here knows about
//! persistence.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteState {
    Raw,
    Cleaned,
    CleanFailed,
    Analyzed,
    ExtractFailed,
}

/// Fixed palette rather than free-form hex: keeps notes inside the Deploy
/// Purple design language and keeps `notes.json` validatable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteColor {
    Purple,
    Violet,
    Amber,
    Teal,
    Rose,
    Slate,
}

impl NoteColor {
    pub fn from_config_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "violet" => Self::Violet,
            "amber" => Self::Amber,
            "teal" => Self::Teal,
            "rose" => Self::Rose,
            "slate" => Self::Slate,
            _ => Self::Purple,
        }
    }

    /// CSS class suffix used by `ui::sticky`.
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Purple => "purple",
            Self::Violet => "violet",
            Self::Amber => "amber",
            Self::Teal => "teal",
            Self::Rose => "rose",
            Self::Slate => "slate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub created: String,
    pub modified: String,
    /// Verbatim transcript or typed text. Never rewritten.
    pub raw: String,
    /// Display text. Equals `raw` until a cleanup pass replaces it.
    pub body: String,
    pub state: NoteState,
    pub color: NoteColor,
    /// **Not read on Linux, and never written from window geometry.**
    ///
    /// Position memory was dropped by decision: `ui::note_layout` chooses where
    /// each note goes, freshly, every launch. The field stays because it is
    /// part of the persisted schema and `with_position` is still honoured
    /// natively on Windows — but nothing captures a window's actual position
    /// into it, and nothing should. Reading a window's own position back is
    /// exactly what Wayland does not permit, and the API that appears to do it
    /// returns `Ok((0, 0))` rather than an error.
    pub pos: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub open: bool,
    pub archived: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_color_name_falls_back_to_purple() {
        assert_eq!(NoteColor::from_config_name("teal"), NoteColor::Teal);
        assert_eq!(
            NoteColor::from_config_name("chartreuse"), NoteColor::Purple,
            "a bad config value must not panic or produce an unrenderable color"
        );
    }
}

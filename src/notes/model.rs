//! Note types. Nothing here knows about persistence.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Extraction progress. `Skipped` means deliberately not run;
/// `Pending` means not run yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    #[default]
    Pending,
    Done,
    Failed,
    Skipped,
}

/// Where a note's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteOrigin {
    /// Must stay the default: older `notes.json` files have no `origin` key.
    #[default]
    Dictated,
    Typed,
}

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
    pub const ALL: [Self; 6] = [
        Self::Purple,
        Self::Violet,
        Self::Amber,
        Self::Teal,
        Self::Rose,
        Self::Slate,
    ];

    pub fn random() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64);
        let mut entropy = now
            ^ COUNTER
                .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
                .rotate_left(17);
        entropy = (entropy ^ (entropy >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        entropy = (entropy ^ (entropy >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        entropy ^= entropy >> 31;

        Self::ALL[entropy as usize % Self::ALL.len()]
    }

    pub fn from_config_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "violet" => Self::Violet,
            "amber" => Self::Amber,
            "teal" => Self::Teal,
            "rose" => Self::Rose,
            "slate" => Self::Slate,
            // The default: every dictated note gets its own color.
            "random" => Self::random(),
            _ => Self::Purple,
        }
    }

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
    /// Display text. Equals `raw` until the user edits it.
    pub body: String,
    /// `#[serde(default)]` so older `notes.json` files (single `"state"` key) still load.
    #[serde(default)]
    pub extract_state: StageState,
    #[serde(default)]
    pub origin: NoteOrigin,
    pub color: NoteColor,
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
        assert!(
            NoteColor::ALL.contains(&NoteColor::from_config_name("random")),
            "random must resolve to a real color"
        );
    }

}

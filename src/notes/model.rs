//! The shape of a note.
//!
//! Split out of `mod.rs` purely for the 500-line limit: the store grew search,
//! archive and restore and pushed the file over. Nothing here knows about
//! persistence.

use serde::{Deserialize, Serialize};

/// How far one model pass has got on a note.
///
/// One field per stage rather than a single linear state: a note can be
/// cleanup-failed *and* extraction-succeeded at once, which a single enum
/// cannot express, and each stage needs its own retry affordance in the footer.
///
/// `Skipped` means deliberately not run — cleanup disabled in config, or a
/// typed note that never needed it. That is not `Pending`, which means "not run
/// yet, and still could be". The UI reads the difference: `Pending` offers a
/// retry, `Skipped` offers nothing to retry.
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
    /// **The default, and it must stay the default.** Every note that predates
    /// this field was born from dictation, so one loaded from an older
    /// `notes.json` with no `origin` key is a dictated note. Defaulting to
    /// `Typed` would silently relabel the entire existing corpus — the labelled
    /// data the eval harness depends on.
    #[default]
    Dictated,
    Typed,
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
    /// Stage fields are `#[serde(default)]` so an older `notes.json` — which
    /// carries a single `"state"` key instead — loads unchanged. `Note` has no
    /// `deny_unknown_fields`, so the stale key is ignored rather than fatal,
    /// and migration stays free. Keep it that way.
    #[serde(default)]
    pub clean_state: StageState,
    #[serde(default)]
    pub extract_state: StageState,
    #[serde(default)]
    pub origin: NoteOrigin,
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

//! The shape of a note.
//!
//! Split out of `mod.rs` purely for the 500-line limit: the store grew search,
//! archive and restore and pushed the file over. Nothing here knows about
//! persistence.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Something a note references, rendered inline where its `[[beamer:<id>]]`
/// token sits in `body`.
///
/// ⚠️ **Paths point at the user's own files and are never copied or deleted.**
/// Beamer does not own an image the way a document editor would: dropping a
/// photo on a note records where that photo lives, and moving the file breaks
/// the reference visibly (`ui::sticky_blocks` renders a missing-file card with
/// a "Locate…" button). That is the deliberate trade — a note is never a second
/// copy of your library, and deleting a note can never delete your photo.
///
/// `#[serde(tag = "kind")]` so `attachments.json`-shaped rows stay readable by
/// eye and a new variant can be added without renumbering anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    Image { id: String, path: PathBuf, alt: Option<String> },
    Link { id: String, url: String, title: Option<String> },
    File { id: String, path: PathBuf },
}

impl Attachment {
    /// The id its token carries.
    pub fn id(&self) -> &str {
        match self {
            Self::Image { id, .. } | Self::Link { id, .. } | Self::File { id, .. } => id,
        }
    }

    /// The file this points at, if it points at one. `None` for a link.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Image { path, .. } | Self::File { path, .. } => Some(path.as_path()),
            Self::Link { .. } => None,
        }
    }

    /// What to call it in the UI: the file name, or the link's host.
    pub fn label(&self) -> String {
        match self {
            Self::Image { path, .. } | Self::File { path, .. } => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            Self::Link { url, title, .. } => match title {
                Some(t) if !t.trim().is_empty() => t.clone(),
                _ => link_label(url),
            },
        }
    }
}

/// A link's host and path, without the scheme — what a chip shows.
///
/// Hand-rolled rather than a URL crate: this is presentation, the input is
/// already known to start with a scheme, and a wrong answer costs a slightly
/// ugly chip rather than a broken link (the `url` field is what is opened).
fn link_label(url: &str) -> String {
    let rest = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .trim_end_matches('/');
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    if rest.is_empty() {
        url.to_string()
    } else {
        rest.to_string()
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
    /// Attachments referenced by `[[beamer:<id>]]` tokens in `body`, in no
    /// particular order — `body` owns reading order.
    ///
    /// `#[serde(default)]`, so a `notes.json` written before attachments
    /// existed loads with an empty vec. Same free-migration mechanism as the
    /// stage fields above; keep it that way.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
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

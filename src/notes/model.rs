//! The shape of a note.
//!
//! Split out of `mod.rs` purely for the 500-line limit: the store grew search,
//! archive and restore and pushed the file over. Nothing here knows about
//! persistence.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Where an attachment's bytes actually are.
///
/// **`Owned` is the normal case, as of this task.** Beamer copies a dropped
/// file's bytes in right away, and from then on all it stores is a sha256 hash
/// and the extension. A hash means the same thing on any machine, which is
/// what makes an attachment syncable at all; a raw path from one OS is
/// meaningless on another, and rewriting it there just breaks it here.
///
/// **`External` is the not-yet-owned case.** It is what a `notes.json` from
/// before this task carries until migration finds the file and copies it in,
/// and it is also what a "Locate…" pick becomes if the chosen file cannot be
/// read. The path may not exist; `ui::sticky_blocks` renders that as a
/// missing-file card with a "Locate…" button rather than dropping the
/// attachment, because a broken reference is recoverable and a deleted one
/// is not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    Owned { hash: String, ext: String },
    External { path: PathBuf },
}

impl Location {
    /// Where to read this location's bytes from, given this machine's
    /// attachments directory.
    ///
    /// Existence is a separate question from what this returns: an `External`
    /// path can be gone, and even an `Owned` file can be, if the local copy
    /// has not synced yet or was removed by hand outside Beamer.
    ///
    /// An `Owned` location whose `hash`/`ext` are not shaped like anything
    /// this code would have written never resolves outside `attachments_dir`.
    /// `hash`/`ext` are plain `String`s straight out of `Deserialize`, and a
    /// hand-edited or (once notes sync) maliciously crafted `notes.json`
    /// could set `hash` to `"../../../home/user/Pictures/cat"`: `Path::join`
    /// discards the base entirely for an absolute-looking second argument,
    /// and `..` traverses without even needing that. `owned_file_name`
    /// rejects anything that is not exactly what `Sha256::digest` and this
    /// codebase's own extension handling produce, and this falls back to a
    /// single fixed name, still inside `attachments_dir`, when it does.
    pub fn resolved_path(&self, attachments_dir: &Path) -> PathBuf {
        match self {
            Self::Owned { hash, ext } => match owned_file_name(hash, ext) {
                Some(name) => attachments_dir.join(name),
                None => attachments_dir.join(INVALID_OWNED_PLACEHOLDER),
            },
            Self::External { path } => path.clone(),
        }
    }
}

/// The fixed name `resolved_path` falls back to for an `Owned` location whose
/// `hash`/`ext` fail `owned_file_name`'s validation. A single hard-coded
/// string, not built from either field, so it can never itself be steered
/// outside `attachments_dir`; nothing legitimate is ever adopted under this
/// name, since `adopt_into` always produces a valid hash.
const INVALID_OWNED_PLACEHOLDER: &str = "invalid-attachment";

/// The `<hash>.<ext>` filename for a content-addressed attachment, or `None`
/// if `hash`/`ext` are not shaped like what this codebase ever writes:
/// `hash` exactly 64 lowercase hex digits (a sha256 hex digest), `ext` 1 to
/// 16 ASCII alphanumeric characters.
///
/// This is the one place that decides whether a `Location::Owned`'s two
/// strings are safe to fold into a path at all. Every filesystem operation
/// keyed on an owned attachment, in this file and in `edit.rs`, must go
/// through this rather than formatting `hash`/`ext` into a path directly.
pub fn owned_file_name(hash: &str, ext: &str) -> Option<String> {
    let hash_ok = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let ext_ok = !ext.is_empty() && ext.len() <= 16 && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    (hash_ok && ext_ok).then(|| format!("{hash}.{ext}"))
}

/// Something a note references, rendered inline where its `[[beamer:<id>]]`
/// token sits in `body`.
///
/// **Beamer owns a copy of what it can.** Dropping a photo on a note copies
/// its bytes into `<config_dir>/sync/attachments`, content-addressed by
/// sha256, and the note records that hash and the original file name rather
/// than a path. Two attachments with identical bytes, even on different
/// notes, share one file on disk; see `edit.rs` for the refcounting that
/// keeps that file around exactly as long as something references it.
///
/// **Deleting a note still never touches your original.** That half of the
/// old design survives unchanged: what gets deleted is Beamer's own copy, the
/// one it made on attach, never the file the photo or document came from.
///
/// `#[serde(tag = "kind")]` so an attachment's row inside `notes.json` stays
/// readable by eye and a new variant can be added without renumbering
/// anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    Image { id: String, filename: String, alt: Option<String>, location: Location },
    Link { id: String, url: String, title: Option<String> },
    File { id: String, filename: String, location: Location },
}

impl Attachment {
    /// The id its token carries.
    pub fn id(&self) -> &str {
        match self {
            Self::Image { id, .. } | Self::Link { id, .. } | Self::File { id, .. } => id,
        }
    }

    /// Where this attachment's bytes are, if it points at bytes at all.
    /// `None` for a link, which has no file.
    pub fn location(&self) -> Option<&Location> {
        match self {
            Self::Image { location, .. } | Self::File { location, .. } => Some(location),
            Self::Link { .. } => None,
        }
    }

    /// Repoint this attachment at a new location. A no-op on a link.
    pub fn set_location(&mut self, new: Location) {
        match self {
            Self::Image { location, .. } | Self::File { location, .. } => *location = new,
            Self::Link { .. } => {}
        }
    }

    /// Rename the display name, e.g. when "Locate…" points at a file that is
    /// not called what the original was. A no-op on a link.
    pub fn set_filename(&mut self, name: String) {
        match self {
            Self::Image { filename, .. } | Self::File { filename, .. } => *filename = name,
            Self::Link { .. } => {}
        }
    }

    /// Where to read this attachment's bytes from, given this machine's
    /// attachments directory. `None` for a link.
    pub fn resolved_path(&self, attachments_dir: &Path) -> Option<PathBuf> {
        self.location().map(|loc| loc.resolved_path(attachments_dir))
    }

    /// What to call it in the UI: the original file name, or the link's host.
    pub fn label(&self) -> String {
        match self {
            Self::Image { filename, .. } | Self::File { filename, .. } => filename.clone(),
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

    #[test]
    fn a_valid_hash_and_extension_resolve_to_the_expected_filename() {
        let hash = "a".repeat(64);
        assert_eq!(owned_file_name(&hash, "png"), Some(format!("{hash}.png")));
    }

    #[test]
    fn a_traversal_shaped_hash_never_resolves_outside_attachments_dir() {
        let attachments_dir = Path::new("/tmp/beamer-attachments-test");
        let cases = [
            Location::Owned { hash: "../../../../etc/passwd".into(), ext: "png".into() },
            Location::Owned { hash: "/home/berkley/Pictures/cat".into(), ext: "png".into() },
            Location::Owned { hash: "a".repeat(64), ext: "../../etc".into() },
            Location::Owned { hash: String::new(), ext: String::new() },
            // One character short of a real digest, easy to get wrong by an
            // off-by-one in a future edit.
            Location::Owned { hash: "a".repeat(63), ext: "png".into() },
            // Uppercase hex is not what `Sha256::digest` formats as; treating
            // it as valid would let two different-looking hashes address the
            // same bytes and confuse the refcount.
            Location::Owned { hash: "A".repeat(64), ext: "png".into() },
        ];
        for location in cases {
            let resolved = location.resolved_path(attachments_dir);
            assert!(
                resolved.starts_with(attachments_dir),
                "{location:?} must resolve inside attachments_dir, got {resolved:?}"
            );
        }
    }

    #[test]
    fn owned_file_name_rejects_what_it_should() {
        let hash = "a".repeat(64);
        assert_eq!(owned_file_name(&hash, ""), None, "an empty extension");
        assert_eq!(owned_file_name(&hash, "p/ng"), None, "a separator in the extension");
        assert_eq!(owned_file_name("../etc/passwd", "png"), None, "a short, traversal-shaped hash");
        assert_eq!(owned_file_name(&"a".repeat(65), "png"), None, "one character too many");
        assert_eq!(owned_file_name(&hash, "png"), Some(format!("{hash}.png")));
    }
}

//! Note types. Nothing here knows about persistence.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// One field per stage: cleanup can fail while extraction succeeds.
/// `Skipped` means deliberately not run; `Pending` means not run yet.
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

    // No non-test caller in this tree yet: the only reader of
    // `notes.default_color` is still in flight. Kept (not deleted)
    // because the parse-fallback contract is pinned by test below.
    #[allow(dead_code)]
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

/// Where an attachment's bytes are. `Owned` is content-addressed (hash + ext);
/// `External` is a not-yet-adopted path, which may not exist (renders as a
/// missing-file card with a "Locate…" button rather than being dropped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    Owned { hash: String, ext: String },
    External { path: PathBuf },
}

impl Location {
    /// Where to read this location's bytes from. `hash`/`ext` are validated
    /// through `owned_file_name` so a crafted `notes.json` cannot escape
    /// `attachments_dir`; invalid shapes fall back to a fixed placeholder name.
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

/// Fixed fallback name for an `Owned` location failing validation. Hard-coded
/// so it can never be steered outside `attachments_dir`.
const INVALID_OWNED_PLACEHOLDER: &str = "invalid-attachment";

/// The `<hash>.<ext>` filename for an owned attachment, or `None` unless `hash`
/// is 64 lowercase hex digits and `ext` is 1–16 ASCII alphanumerics.
/// Every path built from an owned attachment must go through here.
pub fn owned_file_name(hash: &str, ext: &str) -> Option<String> {
    let hash_ok = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let ext_ok = !ext.is_empty() && ext.len() <= 16 && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    (hash_ok && ext_ok).then(|| format!("{hash}.{ext}"))
}

/// Rendered inline where its `[[beamer:<id>]]` token sits in `body`.
/// Beamer owns a content-addressed copy under `attachments_dir` (see `edit.rs`
/// for refcounting); deleting a note only ever removes that copy, never the original.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    Image { id: String, filename: String, alt: Option<String>, location: Location },
    Link { id: String, url: String, title: Option<String> },
    File { id: String, filename: String, location: Location },
}

impl Attachment {
    pub fn id(&self) -> &str {
        match self {
            Self::Image { id, .. } | Self::Link { id, .. } | Self::File { id, .. } => id,
        }
    }

    /// `None` for a link, which has no file.
    pub fn location(&self) -> Option<&Location> {
        match self {
            Self::Image { location, .. } | Self::File { location, .. } => Some(location),
            Self::Link { .. } => None,
        }
    }

    /// No-op on a link.
    pub fn set_location(&mut self, new: Location) {
        match self {
            Self::Image { location, .. } | Self::File { location, .. } => *location = new,
            Self::Link { .. } => {}
        }
    }

    /// No-op on a link.
    pub fn set_filename(&mut self, name: String) {
        match self {
            Self::Image { filename, .. } | Self::File { filename, .. } => *filename = name,
            Self::Link { .. } => {}
        }
    }

    /// `None` for a link.
    pub fn resolved_path(&self, attachments_dir: &Path) -> Option<PathBuf> {
        self.location().map(|loc| loc.resolved_path(attachments_dir))
    }

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

/// A link's host and path without the scheme, for display (the `url` field is what opens).
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
    /// `#[serde(default)]` so older `notes.json` files (single `"state"` key) still load.
    #[serde(default)]
    pub clean_state: StageState,
    #[serde(default)]
    pub extract_state: StageState,
    #[serde(default)]
    pub origin: NoteOrigin,
    pub color: NoteColor,
    /// In no particular order — `body` owns reading order. Defaulted so
    /// pre-attachment `notes.json` files load with an empty vec.
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
            Location::Owned { hash: "a".repeat(63), ext: "png".into() },
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

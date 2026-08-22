//! The shape of an extracted task.
//!
//! Types only — nothing here knows about persistence, mirroring the split
//! between `model.rs` and `mod.rs`.
//!
//! Every row is two things at once: a chip the user acts on, and a labelled
//! example for the eval corpus that will later measure extraction. The second
//! job is why several fields exist that a to-do list would not need.

use serde::{Deserialize, Serialize};

/// Where a proposal stands with the user.
///
/// `Suggested` is not a to-do item yet. Nothing enters a task list unconfirmed
/// — the whole point of the accept/dismiss pair is that the model proposes and
/// the user decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Suggested,
    Accepted,
    Dismissed,
}

/// A row loaded from an older `tasks.json` with no `status` key is a row the
/// user never decided on. Defaulting to `Accepted` would invent confirmations
/// that never happened; defaulting to `Dismissed` would invent rejections. Both
/// are fabricated labels in the corpus, so the undecided state is the only safe
/// default.
impl Default for TaskStatus {
    fn default() -> Self {
        Self::Suggested
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// Provenance. Click a task, get the note it came from — and, when the
    /// extraction misfires, the note is what you need to read to see why.
    pub note_id: String,
    /// Normalized imperative, e.g. "Call the vet". This is what the chip shows.
    pub text: String,
    /// The exact span of the note that produced `text`.
    ///
    /// Not decoration, and not a duplicate of `text`. A false positive is
    /// obvious in one glance when the span that triggered it sits next to it —
    /// "I should probably call the vet sometime" reads very differently as an
    /// extracted commitment than "Call the vet" does alone. It is also the only
    /// thing that makes the prompt debuggable after the fact: without the span,
    /// a bad extraction is a verdict with no evidence, and tuning the prompt
    /// becomes guesswork against a note that may since have been edited.
    pub evidence: String,
    /// Confidence as reported by the model, expected in 0.0-1.0.
    ///
    /// Expected, **not enforced.** A value outside the range means the prompt
    /// or the parser misfired, and that is exactly the anomaly the corpus
    /// should preserve rather than quietly flatten. Clamping belongs at render
    /// time, where it affects a progress bar and nothing else.
    pub confidence: f32,
    #[serde(default)]
    pub status: TaskStatus,
    /// Only meaningful when `status == Accepted`. A suggestion cannot be
    /// completed, because it is not a task yet.
    #[serde(default)]
    pub done: bool,
    /// RFC3339, when the model proposed it.
    pub created: String,
    /// RFC3339, when the user accepted or dismissed it.
    ///
    /// `Option<String>` rather than a bool because the *timestamp* is the eval
    /// signal, not the fact of a decision. Paired with `created` it gives how
    /// long the user looked at the proposal before rejecting it — an instant
    /// dismissal and a considered one are different labels. A dismissed row
    /// with no decision time is a row that cannot be used as a labelled
    /// negative at all, so the two must be recorded together or not at all.
    #[serde(default)]
    pub decided: Option<String>,
}

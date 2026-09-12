//! The backlog sweep's pure decision logic: plain values in, decisions out.
//! No `Signal`, no coroutine, no server. Fired only from `use_pipeline`'s
//! success path, never a timer.

use std::collections::HashSet;

use super::PipelineRequest;
use crate::notes::{NoteStore, StageState};

/// Every note to retry once a success proves the server is reachable again.
/// Archived notes are excluded. No `origin` check: a footer retry already
/// asked, the sweep just carries it.
///
/// `terminal` holds notes whose last failure was terminal (wrong model name,
/// broken server preset): retrying them unchanged can only fail the same
/// way, so they are skipped until a manual retry clears them.
pub(super) fn sweep_requests(
    notes: &NoteStore,
    terminal: &HashSet<String>,
) -> Vec<PipelineRequest> {
    notes
        .notes
        .iter()
        .filter(|note| !note.archived)
        .filter(|note| {
            note.extract_state == StageState::Failed && !terminal.contains(&note.id)
        })
        .map(|note| PipelineRequest { note_id: note.id.clone(), swept: true })
        .collect()
}

/// Sweep only on evidence the server is reachable, and never from a swept
/// request (or failures would re-sweep the backlog forever).
pub(super) fn should_sweep(succeeded: bool, was_swept: bool) -> bool {
    succeeded && !was_swept
}

/// What one attempted pass did. Four states because "no error" is not
/// evidence the server answered (only `Responded` is), and a failure that
/// needs a config or server change first must not be re-swept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestOutcome {
    /// A request went out and the server answered, whatever the answer was.
    Responded,
    /// A request went out and failed transiently: unreachable, timeout, 5xx.
    Errored,
    /// A request went out and failed terminally: 4xx, malformed body, or a
    /// misconfigured server preset. Recorded as `Failed` like any error, but
    /// excluded from the backlog sweep until a manual retry clears it.
    Terminal,
    /// No request went out: disabled pass, missing note, or nothing to send.
    NotAttempted,
}

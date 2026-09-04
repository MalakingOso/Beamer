//! The backlog sweep's pure decision logic: plain values in, decisions out.
//! No `Signal`, no coroutine, no server. Fired only from `use_pipeline`'s
//! success path, never a timer.

use super::{PipelineRequest, Stages};
use crate::notes::{NoteStore, StageState};

/// Every note to retry once a success proves the server is reachable again,
/// asking each only for the stages that failed. Archived notes are excluded.
/// No `origin` check: a footer retry already asked, the sweep just carries it.
pub(super) fn sweep_requests(notes: &NoteStore) -> Vec<PipelineRequest> {
    notes
        .notes
        .iter()
        .filter(|note| !note.archived)
        .filter_map(|note| {
            let stages = match (note.clean_state == StageState::Failed, note.extract_state == StageState::Failed) {
                (true, true) => Some(Stages::Both),
                (true, false) => Some(Stages::CleanOnly),
                (false, true) => Some(Stages::ExtractOnly),
                (false, false) => None,
            };
            stages.map(|stages| PipelineRequest { note_id: note.id.clone(), stages, swept: true })
        })
        .collect()
}

/// Sweep only on evidence the server is reachable, and never from a swept
/// request (or failures would re-sweep the backlog forever).
pub(super) fn should_sweep(succeeded: bool, was_swept: bool) -> bool {
    succeeded && !was_swept
}

/// What one attempted stage did. Three states because "no error" is not
/// evidence the server answered; only `Responded` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestOutcome {
    /// A request went out and the server answered, whatever the answer was.
    Responded,
    /// A request went out and failed: network error, timeout, non-2xx, or a
    /// malformed body.
    Errored,
    /// No request went out: disabled stage, missing note, or nothing to send.
    NotAttempted,
}

/// Fold per-stage outcomes into the sweep trigger: at least one stage got a
/// response and none errored. All-skipped is not success.
pub(super) fn succeeded_from(outcomes: &[RequestOutcome]) -> bool {
    outcomes.contains(&RequestOutcome::Responded) && !outcomes.contains(&RequestOutcome::Errored)
}

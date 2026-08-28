//! The backlog sweep's pure decision logic, split out of `pipeline.rs` to
//! keep that file (the coroutine and the stage-running code) under the
//! 500-line limit.
//!
//! Everything here is a function of plain values (`NoteStore`,
//! `PipelineRequest`, a slice of `RequestOutcome`), with no `Signal`, no
//! coroutine and no server, so `use_pipeline`'s own success path is the only
//! real caller and every branch is covered by `pipeline/tests.rs` instead,
//! reached through `super::*` the same way it reaches the rest of
//! `pipeline.rs`.

use super::{PipelineRequest, Stages};
use crate::notes::{NoteStore, StageState};

/// Every note the pipeline should retry after a success proves the server is
/// reachable again, asking each one only for the stages that actually failed.
///
/// Pure: a function of the store's stage fields and nothing else, so this is
/// testable without a server or a running coroutine. The one caller is
/// `use_pipeline`'s own success path, never a timer. See the module-level
/// warning on never polling the server. `pub(super)`: `use_pipeline` is the
/// only caller, and the test module reaches it through `super::*` regardless.
///
/// Archived notes are excluded. Archiving is the user saying they are done
/// with a note, so a `Failed` stage on one is not a backlog to keep spending
/// requests on; without this filter an archived note with a stuck `Failed`
/// stage would be retried forever, once per success, for as long as the app
/// runs.
///
/// No check on `origin` here: a typed note that reached `Failed` from a
/// footer press is swept the same as a dictated one. That is deliberate, not
/// an oversight. The user already asked once, by pressing retry, and the
/// sweep is only carrying that same ask forward once the server is reachable
/// again, not inventing a new one. See "Trigger policy" in
/// `agent_docs/local_inference.md` for the fuller version of this argument.
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

/// Whether a just-finished request should trigger a backlog sweep.
///
/// The two conditions the brief and the never-poll rule both demand: the pass
/// that just finished actually succeeded (that is the evidence the server is
/// reachable), and it was not itself a swept request (the guard against
/// sweeping forever).
pub(super) fn should_sweep(succeeded: bool, was_swept: bool) -> bool {
    succeeded && !was_swept
}

/// What one attempted stage did, at the granularity the sweep trigger needs.
///
/// Three states, not two, because "no error" is not the same claim as "the
/// server answered". A stage that was never attempted, disabled in config,
/// the whole feature disabled, the note gone before a call could go out, or
/// nothing to send in the first place, produces no evidence about whether
/// the server is reachable, so it must be told apart from a stage that
/// actually got a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestOutcome {
    /// A request went out and the server answered, whatever the answer was.
    /// The only outcome that counts as evidence the server is reachable.
    Responded,
    /// A request went out and failed: network error, timeout, non-2xx, or a
    /// malformed body.
    Errored,
    /// No request went out. Covers a disabled stage, the feature turned off
    /// entirely, a note that vanished before any call, and a stage that had
    /// nothing to send (blank text, an attachment-only note with no runs).
    NotAttempted,
}

/// Fold a pass's per-stage outcomes into the single `succeeded` bool the
/// sweep trigger acts on.
///
/// Pure and deliberately small: at least one stage must have actually gotten
/// a response, and none may have errored. A pass where every stage was
/// skipped or not attempted proves nothing about reachability either way, so
/// it must not read as a success the sweep can build on. That was the bug
/// this replaces, where "no stage errored" alone counted as success even
/// when no stage made a request at all.
pub(super) fn succeeded_from(outcomes: &[RequestOutcome]) -> bool {
    outcomes.contains(&RequestOutcome::Responded) && !outcomes.contains(&RequestOutcome::Errored)
}

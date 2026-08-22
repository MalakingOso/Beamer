//! The model-pass pipeline: one App-scoped coroutine that cleans and analyses
//! notes in the background.
//!
//! **Why a coroutine in `App()` rather than a task spawned where the note is
//! made.** Dioxus drops a spawned task when its owning scope drops
//! (`dioxus-core-0.7.9/src/tasks.rs:159`). A pass started from a sticky
//! window's scope would therefore be **silently cancelled** if you closed that
//! note while the model was still thinking — no error, no log line, just a note
//! that never gets cleaned. `App()`'s scope outlives every note window, so the
//! pass owned by it always finishes.
//!
//! ⚠️ Dioxus `spawn`, never `tokio::spawn`. Desktop's tokio runtime is
//! multi-threaded and `Signal`'s generational-box arena is thread-local, so a
//! `Signal` moved into `tokio::spawn` resolves against the wrong arena.
//!
//! **Requests run concurrently, not in series.** A serial loop would let one
//! hung request stall every later note for up to `request_timeout_ms` — 15
//! seconds by default. In-flight passes are therefore driven together by a
//! `FuturesUnordered` inside the coroutine's single future. That keeps the work
//! owned by `App()`'s scope, which the whole design depends on, while still
//! letting a slow note overlap a fast one; spawning a fresh Dioxus task per
//! request would put ownership back in question for no gain.
//!
//! Duplicate requests for a note already in flight are dropped rather than
//! queued. Two passes over one note would race on the compare-and-swap and the
//! loser's work would be thrown away anyway.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use dioxus::prelude::*;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;

use crate::config::Config;
use crate::llm::cleanup::{self, Cleaned};
use crate::llm::extract;
use crate::notes::lifecycle::StageOutcome;
use crate::notes::task_store::TaskStore;
use crate::notes::{NoteStore, StageState};
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Which stages a request is asking for.
///
/// Separate from "which stages are pending" on purpose: the footer's retry
/// affordance asks for one stage specifically, and re-running a stage that
/// already succeeded is a legitimate thing to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// The single-stage variants are how the note footer retries one pass without
// re-running the other. Constructed once that footer exists.
#[allow(dead_code)]
pub enum Stages {
    /// Cleanup, then extraction against whatever cleanup left behind. The
    /// automatic path after a dictated capture.
    Both,
    CleanOnly,
    ExtractOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineRequest {
    pub note_id: String,
    pub stages: Stages,
}

impl PipelineRequest {
    /// What a freshly dictated note asks for.
    pub fn for_new_note(note_id: impl Into<String>) -> Self {
        Self { note_id: note_id.into(), stages: Stages::Both }
    }
}

/// Start the pipeline. Call once, from `App()`.
pub fn use_pipeline(
    config: Signal<Config>,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    status_log: Signal<StatusLog>,
) -> Coroutine<PipelineRequest> {
    use_coroutine(move |mut rx: UnboundedReceiver<PipelineRequest>| async move {
        // Not a `Signal`: nothing renders from this, and a signal write would
        // wake every subscriber twice per pass for a fact the UI reads off the
        // note's own stage fields instead.
        let in_flight: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
        let mut running = FuturesUnordered::new();

        loop {
            tokio::select! {
                incoming = rx.next() => {
                    let Some(request) = incoming else { break };
                    if !in_flight.borrow_mut().insert(request.note_id.clone()) {
                        tracing::debug!(
                            "pipeline: note {} already in flight, dropping duplicate",
                            request.note_id
                        );
                        continue;
                    }
                    running.push(run_request(request, config, notes, tasks, status_log));
                }
                Some(finished) = running.next(), if !running.is_empty() => {
                    in_flight.borrow_mut().remove(&finished);
                }
            }
        }
    })
}

/// Run one note's requested stages. Returns the note id so the caller can clear
/// it from the in-flight set.
#[allow(clippy::too_many_arguments)]
async fn run_request(
    request: PipelineRequest,
    config: Signal<Config>,
    mut notes: Signal<NoteStore>,
    mut tasks: Signal<TaskStore>,
    mut status_log: Signal<StatusLog>,
) -> String {
    let id = request.note_id.clone();

    // One snapshot, taken up front. `peek`, not `read`: this runs outside any
    // reactive scope and has no business subscribing to the config.
    let (enabled, base_url, timeout, cleanup_cfg, extract_cfg) = {
        let cfg = config.peek();
        (
            cfg.llm.enabled,
            cfg.llm.base_url.clone(),
            Duration::from_millis(cfg.llm.request_timeout_ms),
            cfg.llm.cleanup.clone(),
            cfg.llm.extract.clone(),
        )
    };

    if !enabled {
        // Skipped, not Pending: the user turned the feature off, so there is
        // nothing for a retry affordance to offer.
        //
        // Only a stage that has never run is downgraded. Without the guard, a
        // "Run again" press on a finished note with the feature switched off
        // would rewrite Done as Skipped and erase the record that the passes
        // ever ran — a state change caused entirely by asking for nothing.
        let mut store = notes.write();
        if stage_is_pending(&store, &id, Stage::Clean) {
            store.mark_clean_skipped(&id);
        }
        if stage_is_pending(&store, &id, Stage::Extract) {
            store.mark_extract_skipped(&id);
        }
        return id;
    }

    if matches!(request.stages, Stages::Both | Stages::CleanOnly) {
        if cleanup_cfg.enabled {
            run_cleanup(&id, &base_url, &cleanup_cfg, timeout, &mut notes, &mut status_log).await;
        } else {
            let mut store = notes.write();
            if stage_is_pending(&store, &id, Stage::Clean) {
                store.mark_clean_skipped(&id);
            }
        }
    }

    if matches!(request.stages, Stages::Both | Stages::ExtractOnly) {
        if extract_cfg.enabled {
            run_extraction(
                &id, &base_url, &extract_cfg, timeout, &mut notes, &mut tasks, &mut status_log,
            )
            .await;
        } else {
            let mut store = notes.write();
            if stage_is_pending(&store, &id, Stage::Extract) {
                store.mark_extract_skipped(&id);
            }
        }
    }

    id
}

#[derive(Clone, Copy)]
enum Stage {
    Clean,
    Extract,
}

/// Whether a stage has never run. Guards the `Skipped` downgrades: `Skipped`
/// means "deliberately not run", which is only ever true of a stage that had
/// not run in the first place.
fn stage_is_pending(store: &NoteStore, id: &str, stage: Stage) -> bool {
    store.get(id).is_some_and(|n| {
        let state = match stage {
            Stage::Clean => n.clean_state,
            Stage::Extract => n.extract_state,
        };
        state == StageState::Pending
    })
}

async fn run_cleanup(
    id: &str,
    base_url: &str,
    cfg: &crate::llm::CleanupConfig,
    timeout: Duration,
    notes: &mut Signal<NoteStore>,
    status_log: &mut Signal<StatusLog>,
) {
    // The text the request will carry, captured now. Applying the result is a
    // compare-and-swap against exactly this string — see `lifecycle::apply_cleanup`
    // for why body equality is the guard and `modified` is not.
    let Some(sent) = notes.peek().get(id).map(|n| n.body.clone()) else {
        return;
    };

    match cleanup::clean(base_url, cfg, &sent, timeout).await {
        Ok(outcome) => {
            let text = match &outcome {
                Cleaned::Rewritten(t) => t.as_str(),
                // An empty `cleaned` is how `apply_cleanup` is told "success,
                // change nothing" — it marks the stage Done and leaves the body
                // alone rather than blanking it.
                Cleaned::NothingToChange => "",
            };
            match notes.write().apply_cleanup(id, &sent, text) {
                StageOutcome::Applied => match outcome {
                    Cleaned::Rewritten(_) => {
                        tracing::info!("cleanup rewrote note {}", id);
                    }
                    Cleaned::NothingToChange => {
                        tracing::info!("cleanup found nothing to change in note {}", id);
                    }
                },
                StageOutcome::Superseded => {
                    // Not a failure and not worth a status-log line: the user
                    // typed while the model was thinking, and their text wins.
                    // The stage stays Pending so the footer still offers it.
                    tracing::info!("cleanup for note {} was superseded by an edit", id);
                }
                StageOutcome::NoteGone => {
                    tracing::debug!("note {} disappeared during cleanup", id);
                }
            }
        }
        Err(e) => {
            notes.write().mark_clean_failed(id);
            tracing::warn!("cleanup failed for note {}: {}", id, e);
            log_status(status_log, LogLevel::Error, format!("Note cleanup failed: {e}"));
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_extraction(
    id: &str,
    base_url: &str,
    cfg: &crate::llm::ExtractConfig,
    timeout: Duration,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    status_log: &mut Signal<StatusLog>,
) {
    // Read the body **after** cleanup, not the text cleanup was given. That
    // covers all three outcomes with one line: a cleaned note is analysed as
    // cleaned, a failed cleanup falls back to `raw` (which `body` still equals),
    // and a cleanup superseded by an edit analyses what the user actually
    // typed — which is what they would want looked at.
    let Some(text) = notes.peek().get(id).map(|n| n.body.clone()) else {
        return;
    };
    if text.trim().is_empty() {
        notes.write().mark_analyzed(id);
        return;
    }

    match extract::extract(base_url, cfg, &text, timeout).await {
        Ok(proposals) => {
            let count = proposals.len();
            {
                let mut store = tasks.write();
                let rows = proposals
                    .into_iter()
                    .map(|p| TaskStore::new_suggestion(id, p.text, p.evidence, p.confidence))
                    .collect();
                store.replace_suggestions(id, rows);
                // Written now rather than on a tick. Nothing else flushes this
                // store on a timer — the debounce in `app_setup` is notes-only
                // — so a suggestion left dirty here would live only in memory.
                store.flush_if_dirty();
            }
            // An empty list is a successful answer and the common one. Marking
            // it Done rather than leaving it Pending is what stops the footer
            // nagging forever on every ordinary note.
            notes.write().mark_analyzed(id);
            tracing::info!("extraction proposed {} task(s) for note {}", count, id);
        }
        Err(e) => {
            notes.write().mark_extract_failed(id);
            tracing::warn!("extraction failed for note {}: {}", id, e);
            log_status(status_log, LogLevel::Error, format!("Task extraction failed: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_note_asks_for_both_stages() {
        let req = PipelineRequest::for_new_note("abc");
        assert_eq!(req.note_id, "abc");
        assert_eq!(
            req.stages,
            Stages::Both,
            "dictation is the one path where both passes run unasked"
        );
    }

    #[test]
    fn requests_are_compared_by_note_and_stages() {
        // The in-flight set keys on `note_id` alone, deliberately: a second
        // request for a note already being worked on is a duplicate whatever
        // stages it names, because both would race on the same CAS.
        let a = PipelineRequest { note_id: "n".into(), stages: Stages::Both };
        let b = PipelineRequest { note_id: "n".into(), stages: Stages::CleanOnly };
        assert_ne!(a, b);
        assert_eq!(a.note_id, b.note_id);
    }
}

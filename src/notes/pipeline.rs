//! The model-pass pipeline: one App-scoped coroutine that cleans and analyses notes.
//! Owned by `App()` because Dioxus cancels a task with its owning scope — a pass
//! started from a sticky window would die silently with the window.
//! ⚠️ Dioxus `spawn`, never `tokio::spawn`: `Signal`'s arena is thread-local.
//! In-flight passes run concurrently in one `FuturesUnordered`; duplicates drop.

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
use crate::notes::task::{Proposal, TaskKind};
use crate::notes::task_store::TaskStore;
use crate::notes::{blocks, NoteStore, StageState};
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// The sweep's pure decision logic, split out to keep this file under 500 lines.
#[path = "pipeline/sweep.rs"]
mod sweep;
use sweep::{should_sweep, succeeded_from, sweep_requests, RequestOutcome};

/// Which stages a request asks for. Separate from "pending": retry may
/// legitimately re-run a stage that already succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stages {
    /// Cleanup, then extraction against whatever cleanup left. The automatic path.
    Both,
    CleanOnly,
    ExtractOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineRequest {
    pub note_id: String,
    pub stages: Stages,
    /// Set only by the backlog sweep. A swept completion never triggers another
    /// sweep, or one failing note would re-sweep the backlog forever.
    pub swept: bool,
}

impl PipelineRequest {
    /// What a freshly dictated note asks for.
    pub fn for_new_note(note_id: impl Into<String>) -> Self {
        Self { note_id: note_id.into(), stages: Stages::Both, swept: false }
    }

    /// What the footer's retry affordance asks for. Never swept.
    pub fn retry(note_id: impl Into<String>, stages: Stages) -> Self {
        Self { note_id: note_id.into(), stages, swept: false }
    }
}

/// Start the pipeline. Call once, from `App()`. Returns the request channel
/// and a reactive mirror of which notes are in flight, for the footer's
/// running indicator.
pub fn use_pipeline(
    config: Signal<Config>,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    status_log: Signal<StatusLog>,
) -> (Coroutine<PipelineRequest>, Signal<HashSet<String>>) {
    // Read by the UI, so its membership must be a `Signal`; written at the
    // same call sites as `in_flight` below, which stays the coroutine's own
    // synchronous dedup guard (a `Signal` write is not visible to itself
    // until the next poll, so the guard couldn't rely on it alone).
    let in_flight_signal: Signal<HashSet<String>> = use_signal(HashSet::new);

    let coroutine = use_coroutine(move |mut rx: UnboundedReceiver<PipelineRequest>| async move {
        let in_flight: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
        let mut in_flight_signal = in_flight_signal;
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
                    in_flight_signal.write().insert(request.note_id.clone());
                    running.push(run_request(request, config, notes, tasks, status_log));
                }
                Some(finished) = running.next(), if !running.is_empty() => {
                    in_flight.borrow_mut().remove(&finished.note_id);
                    in_flight_signal.write().remove(&finished.note_id);

                    // A succeeded request is itself the evidence the server is
                    // reachable; have it carry the failed backlog. Never a timer.
                    if should_sweep(finished.succeeded, finished.swept) {
                        let backlog = sweep_requests(&notes.peek());
                        for request in backlog {
                            // `in_flight` still guards here: a mid-retry note is left alone.
                            if in_flight.borrow_mut().insert(request.note_id.clone()) {
                                in_flight_signal.write().insert(request.note_id.clone());
                                running.push(run_request(request, config, notes, tasks, status_log));
                            }
                        }
                    }
                }
            }
        }
    });

    (coroutine, in_flight_signal)
}

/// What one finished pass reports back to the coroutine loop.
struct Finished {
    note_id: String,
    swept: bool,
    succeeded: bool,
}

/// Run one note's requested stages.
async fn run_request(
    request: PipelineRequest,
    config: Signal<Config>,
    mut notes: Signal<NoteStore>,
    mut tasks: Signal<TaskStore>,
    mut status_log: Signal<StatusLog>,
) -> Finished {
    let id = request.note_id.clone();
    let swept = request.swept;

    // One snapshot up front. `peek`, not `read`: no reactive scope here.
    let (enabled, cleanup_base_url, extract_base_url, timeout, cleanup_cfg, extract_cfg) = {
        let cfg = config.peek();
        (
            cfg.llm.enabled,
            cfg.llm.cleanup_base_url().to_string(),
            cfg.llm.extract_base_url().to_string(),
            Duration::from_millis(cfg.llm.request_timeout_ms),
            cfg.llm.cleanup.clone(),
            cfg.llm.extract.clone(),
        )
    };

    if !enabled {
        // Only never-run stages downgrade to Skipped, or "Run again" on a
        // finished note would rewrite Done and erase that the passes ever ran.
        let mut store = notes.write();
        if stage_is_pending(&store, &id, Stage::Clean) {
            store.mark_clean_skipped(&id);
        }
        if stage_is_pending(&store, &id, Stage::Extract) {
            store.mark_extract_skipped(&id);
        }
        // Not `succeeded`: nothing attempted, so no evidence the server is reachable.
        return Finished { note_id: id, swept, succeeded: false };
    }

    // One `RequestOutcome` per named stage, folded by `succeeded_from`, so
    // "disabled"/"nothing to send" can't conflate with "responded".
    let mut outcomes: Vec<RequestOutcome> = Vec::with_capacity(2);

    if matches!(request.stages, Stages::Both | Stages::CleanOnly) {
        if cleanup_cfg.enabled {
            outcomes.push(
                run_cleanup(&id, &cleanup_base_url, &cleanup_cfg, timeout, &mut notes, &mut status_log)
                    .await,
            );
        } else {
            let mut store = notes.write();
            if stage_is_pending(&store, &id, Stage::Clean) {
                store.mark_clean_skipped(&id);
            }
            outcomes.push(RequestOutcome::NotAttempted);
        }
    }

    if matches!(request.stages, Stages::Both | Stages::ExtractOnly) {
        if extract_cfg.enabled {
            outcomes.push(
                run_extraction(
                    &id, &extract_base_url, &extract_cfg, timeout, &mut notes, &mut tasks,
                    &mut status_log,
                )
                .await,
            );
        } else {
            let mut store = notes.write();
            if stage_is_pending(&store, &id, Stage::Extract) {
                store.mark_extract_skipped(&id);
            }
            outcomes.push(RequestOutcome::NotAttempted);
        }
    }

    Finished { note_id: id, swept, succeeded: succeeded_from(&outcomes) }
}

#[derive(Clone, Copy)]
enum Stage {
    Clean,
    Extract,
}

/// Whether a stage has never run. Guards the `Skipped` downgrades.
fn stage_is_pending(store: &NoteStore, id: &str, stage: Stage) -> bool {
    store.get(id).is_some_and(|n| {
        let state = match stage {
            Stage::Clean => n.clean_state,
            Stage::Extract => n.extract_state,
        };
        state == StageState::Pending
    })
}

/// Clean one note, one text run at a time; reassemble and compare-and-swap
/// against the full original body, so a mid-pass edit still supersedes.
/// ⚠️ Placeholder tokens must never reach the model: garbled output comes
/// back at HTTP 200 with nothing to catch downstream. Blank runs are skipped;
/// an error aborts the pass with nothing applied. `NotAttempted` when no call
/// went out (missing note, or only blank runs).
async fn run_cleanup(
    id: &str,
    base_url: &str,
    cfg: &crate::llm::CleanupConfig,
    timeout: Duration,
    notes: &mut Signal<NoteStore>,
    status_log: &mut Signal<StatusLog>,
) -> RequestOutcome {
    // The compare-and-swap baseline: the result applies only against exactly this string.
    let Some(sent) = notes.peek().get(id).map(|n| n.body.clone()) else {
        return RequestOutcome::NotAttempted;
    };

    let runs: Vec<String> = blocks::text_runs(&sent).into_iter().map(str::to_string).collect();
    let mut cleaned: Vec<Option<String>> = Vec::with_capacity(runs.len());
    let mut changed = false;
    // Set only when a call actually goes out; a blank-only body must not read as reachable.
    let mut contacted_server = false;

    for run in &runs {
        if run.trim().is_empty() {
            cleaned.push(None);
            continue;
        }
        contacted_server = true;
        match cleanup::clean(base_url, cfg, run, timeout).await {
            Ok(Cleaned::Rewritten(text)) => {
                changed = true;
                cleaned.push(Some(text));
            }
            Ok(Cleaned::NothingToChange) => cleaned.push(None),
            Err(e) => {
                notes.write().mark_clean_failed(id);
                tracing::warn!("cleanup failed for note {}: {}", id, e);
                log_status(status_log, LogLevel::Error, format!("Note cleanup failed: {e}"));
                return RequestOutcome::Errored;
            }
        }
    }

    // Unchanged passes an empty response, which `apply_cleanup` reads as "success, change nothing".
    let text = if changed { blocks::reassemble(&sent, &cleaned) } else { String::new() };

    match notes.write().apply_cleanup(id, &sent, &text) {
        StageOutcome::Applied => {
            if changed {
                tracing::info!("cleanup rewrote note {} ({} run(s))", id, runs.len());
            } else {
                tracing::info!("cleanup found nothing to change in note {}", id);
            }
        }
        StageOutcome::Superseded => {
            // The user typed mid-pass; their text wins and the stage stays Pending.
            tracing::info!("cleanup for note {} was superseded by an edit", id);
        }
        StageOutcome::NoteGone => {
            tracing::debug!("note {} disappeared during cleanup", id);
        }
    }
    // The loop completed without error here; still only `Responded` if a call went out.
    if contacted_server {
        RequestOutcome::Responded
    } else {
        RequestOutcome::NotAttempted
    }
}

/// Same outcome contract as `run_cleanup`. Blank text is `NotAttempted`:
/// `extract::extract` is never called, so nothing about reachability was learned.
async fn run_extraction(
    id: &str,
    base_url: &str,
    cfg: &crate::llm::ExtractConfig,
    timeout: Duration,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    status_log: &mut Signal<StatusLog>,
) -> RequestOutcome {
    // Read the post-cleanup body: cleaned, or `raw`/the user's edit on the other
    // outcomes. Tokens stripped — evidence spans must never contain token text.
    let Some(text) = notes.peek().get(id).map(|n| blocks::plain_text(&n.body)) else {
        return RequestOutcome::NotAttempted;
    };
    if text.trim().is_empty() {
        notes.write().mark_analyzed(id);
        return RequestOutcome::NotAttempted;
    }

    // Passed in so validation gates stay testable without mocking the clock.
    let today = chrono::Local::now().date_naive();

    match extract::extract(base_url, cfg, &text, today, timeout).await {
        Ok(proposals) => {
            let count = proposals.len();
            {
                let mut store = tasks.write();
                let rows = proposals
                    .into_iter()
                    .map(|p| {
                        TaskStore::new_suggestion(
                            id,
                            Proposal {
                                text: p.text,
                                evidence: p.evidence,
                                confidence: p.confidence,
                                due: p.due,
                                due_all_day: p.due_all_day,
                                due_phrase: p.due_phrase,
                                kind: match p.kind {
                                    extract::TaskKind::Event => TaskKind::Event,
                                    extract::TaskKind::Todo => TaskKind::Todo,
                                },
                            },
                        )
                    })
                    .collect();
                store.replace_suggestions(id, rows);
            }
            // An empty list is a successful answer; marking it Done stops the
            // footer nagging on every ordinary note.
            notes.write().mark_analyzed(id);
            // Through `flush_stores` (the only document writer), after the mark,
            // so one write carries both. `flush_if_dirty` would skip the document.
            crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
            tracing::info!("extraction proposed {} task(s) for note {}", count, id);
            RequestOutcome::Responded
        }
        Err(e) => {
            notes.write().mark_extract_failed(id);
            tracing::warn!("extraction failed for note {}: {}", id, e);
            log_status(status_log, LogLevel::Error, format!("Task extraction failed: {e}"));
            RequestOutcome::Errored
        }
    }
}

#[cfg(test)]
#[path = "pipeline/tests.rs"]
mod tests;

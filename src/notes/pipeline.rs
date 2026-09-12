//! The model-pass pipeline: one App-scoped coroutine that extracts tasks from notes.
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
use crate::llm::extract;
use crate::notes::task::{Proposal, TaskKind};
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// The sweep's pure decision logic, split out to keep this file under 500 lines.
#[path = "pipeline/sweep.rs"]
mod sweep;
use sweep::{should_sweep, sweep_requests, RequestOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineRequest {
    pub note_id: String,
    /// Set only by the backlog sweep. A swept completion never triggers another
    /// sweep, or one failing note would re-sweep the backlog forever.
    pub swept: bool,
}

impl PipelineRequest {
    /// What a freshly dictated note and the footer's retry affordance ask for.
    /// Never swept.
    pub fn new(note_id: impl Into<String>) -> Self {
        Self { note_id: note_id.into(), swept: false }
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
        // Notes whose last failure was terminal: the footer still offers a
        // manual retry, but the sweep leaves them alone until one succeeds.
        // In-memory only — a restart re-attempts once, then the set rebuilds
        // itself from the fresh failures.
        let terminal: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
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
                    if finished.succeeded {
                        // A success clears the terminal record: whatever was
                        // misconfigured evidently is not anymore.
                        terminal.borrow_mut().remove(&finished.note_id);
                    } else if finished.terminal {
                        terminal.borrow_mut().insert(finished.note_id.clone());
                    }

                    // A succeeded request is itself the evidence the server is
                    // reachable; have it carry the failed backlog. Never a timer.
                    if should_sweep(finished.succeeded, finished.swept) {
                        let backlog = sweep_requests(&notes.peek(), &terminal.borrow());
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
    /// Whether the failure was terminal (see `RequestOutcome::Terminal`).
    terminal: bool,
}

/// Run one note's extraction pass.
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
    let (enabled, extract_base_url, timeout, extract_cfg) = {
        let cfg = config.peek();
        (
            cfg.llm.enabled,
            cfg.llm.extract_base_url().to_string(),
            Duration::from_millis(cfg.llm.request_timeout_ms),
            cfg.llm.extract.clone(),
        )
    };

    if !enabled {
        // `mark_extract_skipped` never overwrites `Done`, so this is safe
        // without a Pending guard — and a `Failed` pass going quiet with the
        // feature is correct, not a loss: the sweep must not keep retrying a
        // pass nobody wants run.
        notes.write().mark_extract_skipped(&id);
        // Not `succeeded`: nothing attempted, so no evidence the server is reachable.
        return Finished { note_id: id, swept, succeeded: false, terminal: false };
    }

    if !extract_cfg.enabled {
        notes.write().mark_extract_skipped(&id);
        return Finished { note_id: id, swept, succeeded: false, terminal: false };
    }

    let outcome = run_extraction(
        &id, &extract_base_url, &extract_cfg, timeout, config, &mut notes,
        &mut tasks, &mut status_log,
    )
    .await;

    // "Disabled"/"nothing to send" can't conflate with "responded": only an
    // actual response counts as success.
    Finished {
        note_id: id,
        swept,
        succeeded: outcome == RequestOutcome::Responded,
        terminal: outcome == RequestOutcome::Terminal,
    }
}

/// A result that came back after the user switched the pass off. Writing it
/// would record `Failed` on an error, or `Done` plus task rows on success,
/// for a pass the user explicitly opted out of. `NotAttempted` rather than
/// `Errored`: nothing was learned about the server, and this must not look
/// like a reason to suppress the backlog sweep.
fn abandoned(stage: &str, id: &str) -> RequestOutcome {
    tracing::info!("{} for note {} was abandoned: the pass was switched off mid-flight", stage, id);
    RequestOutcome::NotAttempted
}

/// Blank text is `NotAttempted`: `extract::extract` is never called, so
/// nothing about reachability was learned.
async fn run_extraction(
    id: &str,
    base_url: &str,
    cfg: &crate::llm::ExtractConfig,
    timeout: Duration,
    config: Signal<Config>,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    status_log: &mut Signal<StatusLog>,
) -> RequestOutcome {
    // Read the note body: the transcript, or the user's own edit.
    let Some(text) = notes.peek().get(id).map(|n| n.body.clone()) else {
        return RequestOutcome::NotAttempted;
    };
    if text.trim().is_empty() {
        notes.write().mark_analyzed(id);
        return RequestOutcome::NotAttempted;
    }
    if !config.peek().llm.extract_wanted() {
        return abandoned("extraction", id);
    }

    // Passed in so validation gates stay testable without mocking the clock.
    let today = chrono::Local::now().date_naive();

    match extract::extract(base_url, cfg, &text, today, timeout).await {
        Ok(proposals) => {
            if !config.peek().llm.extract_wanted() {
                // Dropping the proposals is the point: rows written now would
                // appear on the tasks page for a pass the user just retired.
                return abandoned("extraction", id);
            }
            let count = proposals.len();
            // Machine suffix for the new row ids (they sync keyed by id).
            let machine_id = notes.peek().machine_id().to_string();
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
                            &machine_id,
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
            if !config.peek().llm.extract_wanted() {
                return abandoned("extraction", id);
            }
            notes.write().mark_extract_failed(id);
            tracing::warn!("extraction failed for note {}: {}", id, e);
            log_status(status_log, LogLevel::Error, format!("Task extraction failed: {e}"));
            return if e.is_retryable() {
                RequestOutcome::Errored
            } else {
                RequestOutcome::Terminal
            };
        }
    }
}

#[cfg(test)]
#[path = "pipeline/tests.rs"]
mod tests;

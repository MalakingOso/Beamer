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
use crate::notes::task::{Proposal, TaskKind};
use crate::notes::task_store::TaskStore;
use crate::notes::{blocks, NoteStore, StageState};
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Which stages a request is asking for.
///
/// Separate from "which stages are pending" on purpose: the footer's retry
/// affordance asks for one stage specifically, and re-running a stage that
/// already succeeded is a legitimate thing to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Set only by the backlog sweep, see `sweep_requests`. A swept request's
    /// own completion never triggers another sweep, or a note that keeps
    /// failing would re-sweep the whole backlog forever every time any other
    /// note happened to succeed.
    pub swept: bool,
}

impl PipelineRequest {
    /// What a freshly dictated note asks for.
    pub fn for_new_note(note_id: impl Into<String>) -> Self {
        Self { note_id: note_id.into(), stages: Stages::Both, swept: false }
    }

    /// What the footer's retry affordance asks for, and what a fresh request
    /// for a note already worked on asks for. Not swept: a user pressing the
    /// footer is not the backlog sweep, even if it happens to re-request a
    /// stage that previously failed.
    pub fn retry(note_id: impl Into<String>, stages: Stages) -> Self {
        Self { note_id: note_id.into(), stages, swept: false }
    }
}

/// Every note the pipeline should retry after a success proves the server is
/// reachable again, asking each one only for the stages that actually failed.
///
/// Pure: a function of the store's stage fields and nothing else, so this is
/// testable without a server or a running coroutine. The one caller is
/// `use_pipeline`'s own success path, never a timer. See the module-level
/// warning on never polling the server.
pub fn sweep_requests(notes: &NoteStore) -> Vec<PipelineRequest> {
    notes
        .notes
        .iter()
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
fn should_sweep(succeeded: bool, was_swept: bool) -> bool {
    succeeded && !was_swept
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
                    in_flight.borrow_mut().remove(&finished.note_id);

                    // The sweep trigger: a request that just succeeded is
                    // itself the evidence the server is reachable, so ask it
                    // to also carry the rest of the failed backlog. This is
                    // never a timer, see the never-poll warning on
                    // `client::probe`. It fires only from a request that
                    // already completed.
                    if should_sweep(finished.succeeded, finished.swept) {
                        let backlog = sweep_requests(&notes.peek());
                        for request in backlog {
                            // `in_flight` still does its ordinary job here: a
                            // note that is, say, mid-retry from the footer at
                            // the exact moment its sweep would fire is left
                            // alone rather than double-queued.
                            if in_flight.borrow_mut().insert(request.note_id.clone()) {
                                running.push(run_request(request, config, notes, tasks, status_log));
                            }
                        }
                    }
                }
            }
        }
    })
}

/// What one finished pass reports back to the coroutine loop: which note it
/// was, whether it was itself a swept request, and whether it succeeded.
/// That last fact is what the sweep trigger is built on.
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
        // Not `succeeded`: nothing was attempted, so there is no evidence the
        // server is reachable for the sweep to act on.
        return Finished { note_id: id, swept, succeeded: false };
    }

    // Starts true and only ever gets pulled down. A stage that is disabled
    // in config, rather than requested, contacts no server and so cannot make
    // this pass count as failed evidence either way.
    let mut succeeded = true;

    if matches!(request.stages, Stages::Both | Stages::CleanOnly) {
        if cleanup_cfg.enabled {
            if !run_cleanup(&id, &base_url, &cleanup_cfg, timeout, &mut notes, &mut status_log).await {
                succeeded = false;
            }
        } else {
            let mut store = notes.write();
            if stage_is_pending(&store, &id, Stage::Clean) {
                store.mark_clean_skipped(&id);
            }
        }
    }

    if matches!(request.stages, Stages::Both | Stages::ExtractOnly) {
        if extract_cfg.enabled {
            let extract_ok = run_extraction(
                &id, &base_url, &extract_cfg, timeout, &mut notes, &mut tasks, &mut status_log,
            )
            .await;
            if !extract_ok {
                succeeded = false;
            }
        } else {
            let mut store = notes.write();
            if stage_is_pending(&store, &id, Stage::Extract) {
                store.mark_extract_skipped(&id);
            }
        }
    }

    Finished { note_id: id, swept, succeeded }
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

/// Clean one note, one text run at a time.
///
/// ```text
/// body --parse--> [Text a][Attach x][Text b]
///                    |                  |
///               clean(a)            clean(b)   <- tokens are never sent
///                    +------ reassemble ------+
///                                  |
///   apply_cleanup(id, expected = the ORIGINAL FULL body, reassembled)
/// ```
///
/// ⚠️ **A placeholder token must never reach s1-mini.** It is a trained wire
/// format, not a chat model; out-of-distribution input comes back garbled at
/// HTTP 200 with a plausible body, so there is nothing to catch downstream.
/// `blocks::parse` removes the tokens and `blocks::reassemble` puts the answers
/// back at the fixed positions they came from.
///
/// Five rules, each of which preserves an existing behaviour rather than
/// adding one:
///
/// 1. `sent` is still the **whole** body, so the compare-and-swap in
///    `apply_cleanup` is unchanged and an edit mid-pass still supersedes.
/// 2. A blank run is not sent at all; it passes through untouched.
/// 3. A run answering `NothingToChange` keeps its original text.
/// 4. If **every** run had nothing to change, the pass reports that and the
///    body is not rewritten — same as before, and it costs no branch here
///    because `reassemble` returns the body unchanged.
/// 5. **A note with no attachments yields exactly one run**, so it is one call
///    carrying the whole body: today's behaviour, reproduced by construction
///    rather than by a fast-path flag that could get out of step.
///
/// A request that **errors** aborts the pass: the stage is marked failed and
/// nothing is applied. Half a cleaned note is worse than an uncleaned one, and
/// the footer's retry re-runs the whole thing.
///
/// Cost is one call per run — measured at 0.225s each, so a note with two
/// images is ~0.7s. Serial on purpose: concurrency here would buy a fraction of
/// a second and risk reordering the answers.
/// Returns whether the pass completed without a request error, the evidence
/// `should_sweep` acts on. A note that vanished before any request went out
/// counts as `false`: nothing was attempted, so nothing was learned about
/// whether the server is reachable.
async fn run_cleanup(
    id: &str,
    base_url: &str,
    cfg: &crate::llm::CleanupConfig,
    timeout: Duration,
    notes: &mut Signal<NoteStore>,
    status_log: &mut Signal<StatusLog>,
) -> bool {
    // The text the request will carry, captured now. Applying the result is a
    // compare-and-swap against exactly this string — see `lifecycle::apply_cleanup`
    // for why body equality is the guard and `modified` is not.
    let Some(sent) = notes.peek().get(id).map(|n| n.body.clone()) else {
        return false;
    };

    let runs: Vec<String> = blocks::text_runs(&sent).into_iter().map(str::to_string).collect();
    let mut cleaned: Vec<Option<String>> = Vec::with_capacity(runs.len());
    let mut changed = false;

    for run in &runs {
        if run.trim().is_empty() {
            cleaned.push(None);
            continue;
        }
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
                return false;
            }
        }
    }

    // An unchanged reassembly is byte-identical to `sent`, which `apply_cleanup`
    // reads as "success, change nothing" via the same empty-response path it
    // has always had.
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
            // Not a failure and not worth a status-log line: the user typed
            // while the model was thinking, and their text wins. The stage
            // stays Pending so the footer still offers it.
            tracing::info!("cleanup for note {} was superseded by an edit", id);
        }
        StageOutcome::NoteGone => {
            tracing::debug!("note {} disappeared during cleanup", id);
        }
    }
    // Reached only via `Applied`, `Superseded` or `NoteGone`. The request
    // itself got a response in all three; only the mid-flight `Err` above,
    // and the note-vanished-before-any-call guard, return `false`.
    true
}

/// Returns whether the pass completed without a request error, same contract
/// as `run_cleanup`.
async fn run_extraction(
    id: &str,
    base_url: &str,
    cfg: &crate::llm::ExtractConfig,
    timeout: Duration,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    status_log: &mut Signal<StatusLog>,
) -> bool {
    // Read the body **after** cleanup, not the text cleanup was given. That
    // covers all three outcomes with one line: a cleaned note is analysed as
    // cleaned, a failed cleanup falls back to `raw` (which `body` still equals),
    // and a cleanup superseded by an edit analyses what the user actually
    // typed — which is what they would want looked at.
    //
    // Stripped of placeholder tokens, for the same reason cleanup never sends
    // one. `extract::is_grounded` checks evidence against the string it was
    // handed, so this also means an evidence span can never contain token text.
    let Some(text) = notes.peek().get(id).map(|n| blocks::plain_text(&n.body)) else {
        return false;
    };
    if text.trim().is_empty() {
        notes.write().mark_analyzed(id);
        return true;
    }

    // Supplied here rather than read inside `extract`, so the validation gates
    // are testable without mocking the clock.
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
            true
        }
        Err(e) => {
            notes.write().mark_extract_failed(id);
            tracing::warn!("extraction failed for note {}: {}", id, e);
            log_status(status_log, LogLevel::Error, format!("Task extraction failed: {e}"));
            false
        }
    }
}

#[cfg(test)]
#[path = "pipeline/tests.rs"]
mod tests;

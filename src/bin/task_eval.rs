//! Measure the extraction prompt against the user's own accept/dismiss history
//! in `tasks.json` — the corpus is real notes, labelled by the user. Precision
//! is the headline metric (a fabricated task poisons the list); recall is also
//! reported.
//!
//! No `src/lib.rs`, so this binary `#[path]`-includes `llm/` plus the note and
//! task types. `notes/mod.rs` can't be included (it needs `crate::config`),
//! so the JSON envelopes are re-declared locally.
//!
//! ```text
//! cargo run --bin task_eval -- --limit 20
//! ```

// Whole-module includes pull in items this binary never calls.
#![allow(dead_code)]

#[path = "../llm/mod.rs"]
mod llm;
/// Included so image placeholder tokens are stripped exactly as the pipeline
/// strips them — grading against text the model is never sent is meaningless.
#[path = "../notes/blocks.rs"]
mod blocks;
#[path = "../notes/model.rs"]
mod note_model;
#[path = "../notes/task.rs"]
mod task_model;

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;

use llm::chat::ChatError;
use llm::extract::{self, ProposedTask};
use llm::{ExtractConfig, LlmConfig};
use note_model::Note;
use task_model::{Task, TaskStatus};

#[derive(Deserialize)]
struct NotesFile {
    #[serde(default)]
    notes: Vec<Note>,
}

#[derive(Deserialize)]
struct TasksFile {
    #[serde(default)]
    tasks: Vec<Task>,
}

/// How one proposal scored. `Ungraded` (matched no decided row) is never a
/// false positive: the user only judged proposals they were shown, so an
/// unseen-but-correct proposal isn't wrong. Printed, counted, excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    TruePositive,
    FalsePositive,
    Ungraded,
}

impl Verdict {
    fn tag(self) -> &'static str {
        match self {
            Self::TruePositive => "TP      ",
            Self::FalsePositive => "FP      ",
            Self::Ungraded => "UNGRADED",
        }
    }
}

#[derive(Default)]
struct Totals {
    tp: usize,
    fp: usize,
    fn_: usize,
    ungraded: usize,
    graded_notes: usize,
    errored_notes: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Groundedness warnings are signal here (invented evidence is what this
    // harness catches), so install a subscriber to surface them.
    tracing_subscriber::fmt()
        .with_env_filter("beamer=warn,task_eval=info,warn")
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let defaults = LlmConfig::default();

    let notes_path = arg(&args, "--notes")
        .map(PathBuf::from)
        .unwrap_or(config_dir()?.join("notes.json"));
    let tasks_path = arg(&args, "--tasks")
        .map(PathBuf::from)
        .unwrap_or(config_dir()?.join("tasks.json"));
    let base_url = arg(&args, "--base-url").unwrap_or(defaults.base_url.clone());
    let limit = match arg(&args, "--limit") {
        Some(s) => Some(s.parse::<usize>().context("--limit takes a number")?),
        None => None,
    };

    let mut cfg = ExtractConfig::default();
    if let Some(model) = arg(&args, "--model") {
        cfg.model = model;
    }
    if let Some(mc) = arg(&args, "--min-confidence") {
        cfg.min_confidence = mc.parse::<f32>().context("--min-confidence takes a float")?;
    }
    let timeout = Duration::from_millis(defaults.request_timeout_ms);

    let notes = read_json::<NotesFile>(&notes_path)?.map(|f| f.notes);
    let tasks = read_json::<TasksFile>(&tasks_path)?.map(|f| f.tasks);

    println!("model        {}", cfg.model);
    println!("base-url     {base_url}");
    println!("min-conf     {:.2}", cfg.min_confidence);
    println!("notes        {}", notes_path.display());
    println!("tasks        {}", tasks_path.display());
    println!();

    let Some(notes) = notes else {
        println!(
            "No notes file at {}. Dictate a note or two first — the corpus is \
             your own notes, not a fixture.",
            notes_path.display()
        );
        return Ok(());
    };
    let Some(tasks) = tasks else {
        println!(
            "No tasks file at {}.\n\
             Nothing proposed or decided yet, so no labels to measure against. \
             Capture notes, accept or dismiss suggestions, then rerun.",
            tasks_path.display()
        );
        return Ok(());
    };

    // Gradeable = someone judged a row on it (archived notes count).
    let mut gradeable: Vec<(&Note, Vec<&Task>)> = Vec::new();
    let mut skipped = 0usize;
    for note in &notes {
        let decided: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.note_id == note.id)
            .filter(|t| matches!(t.status, TaskStatus::Accepted | TaskStatus::Dismissed))
            .collect();
        if decided.is_empty() {
            skipped += 1;
        } else {
            gradeable.push((note, decided));
        }
    }

    if gradeable.is_empty() {
        let undecided = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Suggested)
            .count();
        println!(
            "No gradeable notes ({} notes, {} rows, {} still Suggested). \
             Accept or dismiss some suggestions and run again.",
            notes.len(),
            tasks.len(),
            undecided
        );
        return Ok(());
    }

    // `--limit` caps notes run; only gradeable notes are ever run.
    let total = gradeable.len();
    let planned = limit.map(|n| n.min(total)).unwrap_or(total);
    println!(
        "{planned} of {total} gradeable note(s) to run; {skipped} skipped for want of labels.\n"
    );

    let mut totals = Totals::default();
    let started = Instant::now();

    for (i, (note, decided)) in gradeable.iter().take(planned).enumerate() {
        print!("── [{}/{planned}] {} ", i + 1, note.id);
        println!("{}", "─".repeat(46usize.saturating_sub(note.id.len())));
        // What the pipeline sends: tokens stripped, never escaped.
        let sent = blocks::plain_text(&note.body);
        for line in sent.trim().lines() {
            println!("   │ {line}");
        }
        let _ = std::io::stdout().flush();

        // Relative dates resolve against capture day, not eval day.
        let today = chrono::DateTime::parse_from_rfc3339(&note.created)
            .map(|dt| dt.with_timezone(&chrono::Local).date_naive())
            .unwrap_or_else(|_| chrono::Local::now().date_naive());

        let t0 = Instant::now();
        let proposals = extract::extract(&base_url, &cfg, &sent, today, timeout).await;
        let elapsed = t0.elapsed();

        let proposals = match proposals {
            Ok(p) => p,
            Err(e @ (ChatError::Unreachable(_) | ChatError::Http(_))) => {
                // Dead server / unserved model fails identically for all notes.
                println!("   ✖ {e}\n");
                println!("Aborting: this failure repeats for every note.");
                break;
            }
            Err(e) => {
                // Per-note failures: counted, kept out of metrics, run continues.
                println!("   ✖ {e}  ({:.2}s)\n", elapsed.as_secs_f64());
                totals.errored_notes += 1;
                continue;
            }
        };

        totals.graded_notes += 1;
        println!(
            "   → {} proposal(s) in {:.2}s, against {} decided row(s)",
            proposals.len(),
            elapsed.as_secs_f64(),
            decided.len()
        );

        let mut taken = vec![false; decided.len()];
        for p in &proposals {
            let verdict = match match_row(p, decided, &taken) {
                Some(idx) => {
                    taken[idx] = true;
                    match decided[idx].status {
                        TaskStatus::Accepted => {
                            totals.tp += 1;
                            Verdict::TruePositive
                        }
                        _ => {
                            totals.fp += 1;
                            Verdict::FalsePositive
                        }
                    }
                }
                None => {
                    totals.ungraded += 1;
                    Verdict::Ungraded
                }
            };
            println!("   {}  {}  ({:.2})", verdict.tag(), p.text, p.confidence);
            println!("             ⤷ {:?}", p.evidence);
        }

        for (idx, row) in decided.iter().enumerate() {
            if !taken[idx] && row.status == TaskStatus::Accepted {
                totals.fn_ += 1;
                println!("   FN        {}", row.text);
                println!("             ⤷ {:?}", row.evidence);
            }
        }
        println!();
    }

    let wall = started.elapsed();
    print_summary(&totals, skipped, wall);
    Ok(())
}

/// Match one proposal to a decided row, consuming each row at most once. Text
/// first (normalized: trim, collapse whitespace, lowercase — the model doesn't
/// reproduce its own wording across runs), then the evidence span, which is
/// quoted from the note and therefore stabler. Evidence is second because two
/// tasks from one sentence share a span and could cross-match; consumed rows
/// bound that to a sibling pairing, never an invented match.
fn match_row(p: &ProposedTask, decided: &[&Task], taken: &[bool]) -> Option<usize> {
    let text = norm(&p.text);
    if !text.is_empty() {
        for (i, row) in decided.iter().enumerate() {
            if !taken[i] && norm(&row.text) == text {
                return Some(i);
            }
        }
    }
    let evidence = norm(&p.evidence);
    if evidence.is_empty() {
        return None;
    }
    for (i, row) in decided.iter().enumerate() {
        if !taken[i] && norm(&row.evidence) == evidence {
            return Some(i);
        }
    }
    None
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn print_summary(t: &Totals, skipped: usize, wall: Duration) {
    let judged = t.tp + t.fp;
    let accepted = t.tp + t.fn_;
    let precision = ratio(t.tp, judged);
    let recall = ratio(t.tp, accepted);
    let mean = if t.graded_notes > 0 {
        wall.as_secs_f64() / t.graded_notes as f64
    } else {
        0.0
    };

    println!("════════════════════════════════════════════════════════");
    println!("  precision  {precision}   ({} TP / {judged} judged proposals)", t.tp);
    println!("  recall     {recall}   ({} TP / {accepted} accepted rows)", t.tp);
    println!();
    println!(
        "  TP {}   FP {}   FN {}   UNGRADED {}",
        t.tp, t.fp, t.fn_, t.ungraded
    );
    println!(
        "  notes graded {}   errored {}   skipped for want of labels {skipped}",
        t.graded_notes, t.errored_notes
    );
    println!(
        "  wall clock {:.2}s   mean {mean:.2}s/note",
        wall.as_secs_f64()
    );
    println!();
    println!(
        "  UNGRADED proposals are excluded from precision and recall. They\n\
         \x20 matched no decided row, which means the user never saw them — not\n\
         \x20 that they are wrong. Judge them by reading the diffs above; each\n\
         \x20 one you decide in the app becomes a label the next run can use."
    );
    println!("════════════════════════════════════════════════════════");
}

/// Percentage, or "n/a" when the denominator is zero (no measurement, not 0%).
/// Both branches are six columns wide so the rates align.
fn ratio(num: usize, den: usize) -> String {
    if den == 0 {
        "   n/a".to_string()
    } else {
        format!("{:5.1}%", 100.0 * num as f64 / den as f64)
    }
}

/// `Ok(None)` for a missing file (ordinary pre-first-decision state). An
/// existing-but-unparseable file is an error: treating it as empty would
/// report "no labels yet" over a corpus that's sitting right there.
fn read_json<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Could not read {}", path.display()))?;
    let parsed = serde_json::from_str(&text)
        .with_context(|| format!("Could not parse {}", path.display()))?;
    Ok(Some(parsed))
}

/// Mirrors `Config::config_dir()`, which lives in an un-includable module.
fn config_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Could not determine the config directory")?
        .join("Beamer"))
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

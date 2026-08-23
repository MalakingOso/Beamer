//! `task_eval` — measure the extraction prompt against the user's own notes.
//!
//! There is no synthetic benchmark here and there never will be. Every chip the
//! user accepts or dismisses is written to `tasks.json` and **kept**, so the
//! accumulated decisions *are* the corpus: real notes, in the user's own voice,
//! labelled by the only person whose judgment the feature answers to.
//!
//! Extraction is a **precision** problem. A fabricated task poisons a list
//! nobody can un-poison, while a missed one costs a re-read. Recall is reported
//! because it is cheap to report, but precision is the headline and the number
//! to tune against.
//!
//! ## Why this file looks the way it does
//!
//! There is no `src/lib.rs`; every module lives under the `beamer` binary, so a
//! second binary cannot `use beamer::…`. `src/llm/**` was deliberately written
//! free of crate-rooted paths so it can be `#[path]`-included here instead —
//! see the module note in `llm/prompts.rs`. `src/notes/mod.rs` is *not*
//! includable (it reaches for `crate::config::Config` to find the storage
//! path), so the two JSON files are read here with local envelope structs that
//! match what the stores write: `{"notes": […]}` and `{"tasks": […]}`, the path
//! and dirty fields being `#[serde(skip)]` on both sides.
//!
//! ## Run it
//!
//! ```text
//! cargo run --bin task_eval -- --limit 20
//! cargo run --bin task_eval -- --model gemma-4-E2B_q4_0-it   # walk the ladder
//! ```

// The `#[path]`-included modules bring in the whole LLM client and both note
// types, of which this binary calls a handful of items. That is exactly the
// situation a file-level allow is for: the alternative is scattering `#[allow]`
// through shared source to suit one consumer, and the tree is held at zero
// warnings.
#![allow(dead_code)]

#[path = "../llm/mod.rs"]
mod llm;
/// The placeholder-token grammar. Pure std with no crate-rooted paths, so it
/// includes cleanly — and it has to be here, because a note holding an image
/// carries `[[beamer:…]]` in its body and grading the model against text it is
/// never sent would measure the wrong thing.
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

/// How one proposal scored against the decided rows for its note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    TruePositive,
    FalsePositive,
    /// The proposal matched no decided row.
    ///
    /// ⚠️ **Never scored as a false positive.** The user only ever judged the
    /// proposals they were *shown*, on the run that produced them. A proposal
    /// that appears now and did not appear then may be perfectly correct and
    /// simply unseen. Counting it as wrong would bias the harness in the
    /// direction that looks like rigour — reporting a worse number than the
    /// evidence supports — and the whole point of the corpus is that it only
    /// contains labels a human actually applied. Ungraded rows are printed,
    /// counted, and excluded from both metrics.
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
    // The extraction parser drops ungrounded proposals with `tracing::warn!`.
    // For an eval run those drops are signal — a model inventing evidence is
    // precisely what this harness exists to catch — so they must not vanish
    // into an uninstalled subscriber.
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
        // The first person to run this lands here, and it is not an error.
        println!(
            "No tasks file at {}.\n\
             Nothing has been proposed or decided yet, so there are no labels to \
             measure against. Capture a few notes, let extraction run, then accept \
             or dismiss the suggestion chips — each decision becomes one labelled \
             example and this harness starts having something to say.",
            tasks_path.display()
        );
        return Ok(());
    };

    // A note is gradeable only if someone has actually judged a row on it.
    // Archived notes are included: a decision does not expire when the note is
    // filed away.
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
            "No gradeable notes.\n\
             {} note(s) on file, {} task row(s), of which {} are still Suggested \
             and none are Accepted or Dismissed for a note that exists.\n\
             A note nobody has judged carries no labels, so grading it would \
             measure nothing. Accept or dismiss some suggestions and run again.",
            notes.len(),
            tasks.len(),
            undecided
        );
        return Ok(());
    }

    // `--limit` caps the notes actually *run*, and only gradeable notes are ever
    // run — spending a model pass on a note with no labels tells you nothing.
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
        // Exactly what the pipeline sends: tokens stripped, never escaped.
        let sent = blocks::plain_text(&note.body);
        for line in sent.trim().lines() {
            println!("   │ {line}");
        }
        let _ = std::io::stdout().flush();

        // The day the note was **captured**, not the day the eval runs. A note
        // saying "before Friday" only ever meant a Friday relative to when it
        // was spoken; grading it against today would measure nothing.
        let today = chrono::DateTime::parse_from_rfc3339(&note.created)
            .map(|dt| dt.with_timezone(&chrono::Local).date_naive())
            .unwrap_or_else(|_| chrono::Local::now().date_naive());

        let t0 = Instant::now();
        let proposals = extract::extract(&base_url, &cfg, &sent, today, timeout).await;
        let elapsed = t0.elapsed();

        let proposals = match proposals {
            Ok(p) => p,
            Err(e @ (ChatError::Unreachable(_) | ChatError::Http(_))) => {
                // Both fail identically for every remaining note — a dead server
                // or a model id the server does not serve. Grinding through the
                // rest would produce a wall of the same message and a summary
                // computed over nothing.
                println!("   ✖ {e}\n");
                println!("Aborting: this failure repeats for every note.");
                break;
            }
            Err(e) => {
                // Malformed output and thinking-left-on are per-note facts worth
                // seeing individually, so the run continues; they are counted
                // and kept out of both metrics.
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

/// Match one proposal to a decided row, consuming each row at most once.
///
/// Two passes. **Text first**, on a normalized form (trim, collapse whitespace,
/// lowercase) — byte equality is useless here because the model does not
/// reproduce its own imperative across runs; "Call the vet" and "call the  vet"
/// are the same decision.
///
/// **Evidence span as a fallback**, normalized the same way. Chosen because the
/// span is *quoted from the note* rather than composed, so it is far more stable
/// across runs than the rewritten imperative — a model that re-words "Call the
/// vet" to "Phone the vet about Milo" still quotes the same sentence. The risk
/// is the mirror image: two genuinely different tasks drawn from one sentence
/// share a span and could cross-match. That is why the pass is second and why
/// rows are consumed — a span collision can at worst pair a proposal with a
/// sibling row from the same sentence, never invent a match where the note said
/// nothing. Preferring evidence first would make the collision the common case.
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

/// Percentages, with "n/a" rather than a fabricated 0% when the denominator is
/// zero. A run with no judged proposals has not measured 0% precision; it has
/// not measured precision. Both branches are six columns wide so the two rates
/// line up under each other.
fn ratio(num: usize, den: usize) -> String {
    if den == 0 {
        "   n/a".to_string()
    } else {
        format!("{:5.1}%", 100.0 * num as f64 / den as f64)
    }
}

/// `Ok(None)` when the file is simply absent — the ordinary state before the
/// first decision, and a message rather than an error. A file that exists and
/// does not parse *is* an error: silently treating it as empty would report a
/// clean "no labels yet" over a corpus that is sitting right there.
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

/// `dirs` directly rather than `Config::config_dir()`, which lives in a module
/// this binary cannot include. Kept identical to it on purpose.
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

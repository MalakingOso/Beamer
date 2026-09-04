//! Stage 2 — task extraction with Gemma. A precision problem: a fabricated
//! task poisons a list nobody then trusts, so every check here assumes the
//! model misbehaves. Parse-time gates: fences tolerated, evidence must ground
//! in the sent note, confidence floor applied before the store. Date gates in
//! `resolve_date` downgrade (drop the date, keep the task), never discard.

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, NaiveDateTime};
use serde::Deserialize;

use super::chat::{self, ChatError, ChatRequest, Message, ResponseFormat};
use super::prompts;
use super::ExtractConfig;

/// Whether a dated task is a deadline or an appointment. Separate from the
/// store's own kind type: this module cannot reach the store, which owns the
/// persisted shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskKind {
    #[default]
    Todo,
    Event,
}

/// One proposed task, after grounding and the confidence floor. The model's
/// half of the record only — the store owns identity, status and timestamps.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedTask {
    pub text: String,
    /// Span of the note that produced this task, verbatim.
    pub evidence: String,
    pub confidence: f32,
    /// `YYYY-MM-DD` when `due_all_day`, else `YYYY-MM-DDTHH:MM:SS`. `None`
    /// when the model found no date, or one that failed a gate.
    pub due: Option<String>,
    pub due_all_day: bool,
    /// Verbatim span the date was read from; survives when `due` does not, so
    /// the UI can offer a picker.
    pub due_phrase: Option<String>,
    pub kind: TaskKind,
}

#[derive(Deserialize)]
struct TaskEnvelope {
    #[serde(default)]
    tasks: Vec<RawTask>,
}

#[derive(Deserialize)]
struct RawTask {
    #[serde(default)]
    text: String,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    due: Option<String>,
    /// The model's `due_all_day` flag is ignored: day-vs-moment is decided by
    /// the shape of `due` in `parse_due`, since the two can contradict.
    #[serde(default)]
    due_phrase: Option<String>,
    /// Free text, not an enum: an unrecognised value degrades to `Todo`, and a
    /// strict enum would drop every task in the envelope on one bad `kind`.
    #[serde(default)]
    kind: Option<String>,
}

/// A validated date, or as much of one as survived.
#[derive(Debug, Clone, PartialEq, Default)]
struct ResolvedDate {
    due: Option<String>,
    all_day: bool,
    phrase: Option<String>,
}

/// Apply the date gates; never fails — worst outcome is no date. `due_phrase`
/// must ground in the note, `due` must parse, and `due` may be at most a day
/// in the past (one day of slack for just-after-midnight passes). `today` is a
/// parameter so the gates are testable without mocking time.
fn resolve_date(raw: &RawTask, note: &str, today: NaiveDate) -> ResolvedDate {
    let phrase = raw
        .due_phrase
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let ok = is_grounded(p, note);
            if !ok {
                tracing::warn!(
                    "extraction: dropped ungrounded due_phrase {:?} on {:?}",
                    p, raw.text
                );
            }
            ok
        })
        .map(str::to_string);

    if phrase.is_none() {
        return ResolvedDate::default();
    }

    let Some(due) = raw.due.as_deref().map(str::trim).filter(|d| !d.is_empty()) else {
        return ResolvedDate { due: None, all_day: false, phrase };
    };

    let Some((normalized, all_day, day)) = parse_due(due) else {
        tracing::warn!("extraction: {:?} is not a date, keeping the phrase only", due);
        return ResolvedDate { due: None, all_day: false, phrase };
    };

    if day < today - ChronoDuration::days(1) {
        tracing::warn!(
            "extraction: {:?} resolved to {}, which is in the past — keeping the phrase only",
            phrase, day
        );
        return ResolvedDate { due: None, all_day: false, phrase };
    }

    ResolvedDate { due: Some(normalized), all_day, phrase }
}

/// Parse the shapes the model is asked for, plus RFC3339 in case it adds an
/// offset. Returns the normalized value, whether it names a whole day, and
/// the day it falls on.
fn parse_due(due: &str) -> Option<(String, bool, NaiveDate)> {
    if let Ok(date) = NaiveDate::parse_from_str(due, "%Y-%m-%d") {
        return Some((date.format("%Y-%m-%d").to_string(), true, date));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(due) {
        let local = dt.with_timezone(&chrono::Local).naive_local();
        return Some((local.format("%Y-%m-%dT%H:%M:%S").to_string(), false, local.date()));
    }
    let dt = NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M"))
        .ok()?;
    Some((dt.format("%Y-%m-%dT%H:%M:%S").to_string(), false, dt.date()))
}

/// Anything the prompt did not ask for is a to-do.
fn parse_kind(kind: Option<&str>) -> TaskKind {
    match kind.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("event") => TaskKind::Event,
        Some("todo") | None => TaskKind::Todo,
        Some(other) => {
            tracing::debug!("extraction: unrecognised kind {:?}, treating as a to-do", other);
            TaskKind::Todo
        }
    }
}

/// Strip a markdown code fence if present. Tolerant: an unterminated fence
/// still yields the body, so a truncated response keeps a usable first task.
pub fn strip_fences(body: &str) -> &str {
    let trimmed = body.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = match rest.find('\n') {
        Some(nl) => &rest[nl + 1..],
        None => rest,
    };
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

/// Strip a leaked reasoning block: some servers leave `reasoning_content` in
/// `content` (a llama.cpp fork's tag detection misses some tag variants), so
/// it arrives as trace + closing tag + JSON. Take everything after the last
/// `</...think...>`-shaped tag; tagless bodies pass through unchanged.
pub fn strip_think_tags(body: &str) -> &str {
    let lower = body.to_ascii_lowercase();
    let mut last_end = None;
    let mut search_from = 0;
    while let Some(open_rel) = lower[search_from..].find("</") {
        let open = search_from + open_rel;
        let Some(close_rel) = lower[open..].find('>') else { break };
        let close = open + close_rel;
        if lower[open..close].contains("think") {
            last_end = Some(close + 1);
        }
        search_from = close + 1;
    }
    match last_end {
        Some(end) => body[end..].trim_start(),
        None => body,
    }
}

/// Case- and whitespace-insensitive containment: transcripts carry newlines a
/// quoting model will not reproduce. Normalizing this far cannot conjure a
/// sentence the note does not contain.
fn is_grounded(evidence: &str, note: &str) -> bool {
    // Empty spans ground nowhere: every string contains "".
    if evidence.trim().is_empty() {
        return false;
    }
    normalize(note).contains(&normalize(evidence))
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Turn a completion body into the proposals worth showing. Grounding is
/// checked against `note`, the text the request carried — not the note as it
/// reads now. An empty list is a successful, frequent answer.
pub fn parse_tasks(
    body: &str,
    note: &str,
    min_confidence: f32,
    today: NaiveDate,
) -> Result<Vec<ProposedTask>, ChatError> {
    let json = strip_fences(strip_think_tags(body));
    let envelope: TaskEnvelope =
        serde_json::from_str(json).map_err(|e| ChatError::Malformed(e.to_string()))?;

    let mut kept = Vec::new();
    for raw in envelope.tasks {
        if raw.text.trim().is_empty() {
            tracing::debug!("extraction: dropped a proposal with no text");
            continue;
        }
        if !is_grounded(&raw.evidence, note) {
            tracing::warn!(
                "extraction: dropped ungrounded proposal {:?} (evidence {:?} is not in the note)",
                raw.text,
                raw.evidence
            );
            continue;
        }
        // Clamped, not rejected: 1.2 is miscalibrated, not a lie about the task.
        let confidence = raw.confidence.clamp(0.0, 1.0);
        if confidence < min_confidence {
            tracing::debug!("extraction: dropped {:?} below the floor", raw.text);
            continue;
        }
        // Resolved last: the date can only ever remove itself.
        let date = resolve_date(&raw, note, today);
        kept.push(ProposedTask {
            text: raw.text.trim().to_string(),
            evidence: raw.evidence.trim().to_string(),
            confidence,
            due: date.due,
            due_all_day: date.all_day,
            due_phrase: date.phrase,
            kind: parse_kind(raw.kind.as_deref()),
        });
    }
    Ok(kept)
}

/// Build the request for one note. Asks for `json_object`, though
/// `strip_fences` stays: not every server honours the grammar constraint.
/// `today` goes in the system message; the user message carries the note byte
/// for byte.
pub fn build_request(cfg: &ExtractConfig, note: &str, today: NaiveDate) -> ChatRequest {
    ChatRequest {
        model: cfg.model.clone(),
        messages: vec![
            Message::system(prompts::extract_system(today)),
            Message::user(note),
        ],
        response_format: Some(ResponseFormat::json_object()),
    }
}

/// Run one extraction pass. `today` is a parameter so gates stay testable and
/// `task_eval` can grade a note against its capture day.
pub async fn extract(
    base_url: &str,
    cfg: &ExtractConfig,
    note: &str,
    today: NaiveDate,
    timeout: Duration,
) -> Result<Vec<ProposedTask>, ChatError> {
    let request = build_request(cfg, note, today);
    let body = chat::complete(base_url, &request, timeout).await?;
    parse_tasks(&body, note, cfg.min_confidence, today)
}

#[cfg(test)]
#[path = "extract/tests.rs"]
mod tests;

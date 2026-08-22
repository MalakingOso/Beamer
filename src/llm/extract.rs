//! Stage 2 — task extraction with Gemma.
//!
//! Extraction is a **precision** problem, not an extraction problem: a
//! fabricated task is worse than a missed one, because a list nobody trusts
//! cannot be un-poisoned. The prompt in `prompts.rs` does what a prompt can;
//! everything here is the part that does not depend on the model behaving.
//!
//! Three checks, all of them at parse time:
//!
//! 1. **Fences are tolerated.** Models emit ```` ```json ```` regardless of
//!    instructions, and the server's `json_object` grammar constraint is not
//!    honoured by every version.
//! 2. **Evidence must be present in the note.** A proposal whose evidence span
//!    is not in the text that was actually sent is a fabrication and is
//!    dropped. This is the one check that does not care how good the model is.
//! 3. **The confidence floor is applied here, not in the UI.** A row that is
//!    never shown to anyone is not a labelled example and must not reach
//!    `tasks.json`, where it would pollute the eval corpus with a decision
//!    nobody made.

use std::time::Duration;

use serde::Deserialize;

use super::chat::{self, ChatError, ChatRequest, Message, ResponseFormat};
use super::prompts;
use super::ExtractConfig;

/// One task the model proposed, after grounding and the confidence floor.
///
/// Deliberately not `notes::task::Task`: this module cannot reach into the
/// crate (see the module rule in `prompts.rs`), and the store owns identity,
/// status and timestamps. This is the model's half of the record and nothing
/// more.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedTask {
    pub text: String,
    /// The span of the note that produced this task, verbatim. What makes a
    /// false positive visible in one glance.
    pub evidence: String,
    pub confidence: f32,
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
}

/// Strip a markdown code fence, if the model wrapped its answer in one.
///
/// Tolerant rather than strict: an unterminated fence still yields the body,
/// because a truncated response with a usable first task is worth more than an
/// error.
pub fn strip_fences(body: &str) -> &str {
    let trimmed = body.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop the info string (```json, ```JSON, ``` on its own) up to the newline.
    let rest = match rest.find('\n') {
        Some(nl) => &rest[nl + 1..],
        None => rest,
    };
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

/// Case- and whitespace-insensitive containment.
///
/// Not a raw `contains`. A transcript carries newlines and doubled spaces that
/// a model quoting from it will not reproduce exactly, and rejecting a genuine
/// span over a capital letter would make the grounding check look broken while
/// being useless. Normalizing this far cannot turn a sentence the note does not
/// contain into one it does, so the guard keeps its teeth.
fn is_grounded(evidence: &str, note: &str) -> bool {
    // An empty span is not grounding — every string contains the empty string,
    // so without this check a model that omitted `evidence` entirely would sail
    // through the one test that does not depend on its judgment.
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

/// Turn a completion body into the proposals worth showing.
///
/// `note` is the text the request actually carried — grounding is checked
/// against that, not against whatever the note says by the time this returns.
///
/// An empty list is a **successful** answer and a frequent one; most notes
/// contain no tasks.
pub fn parse_tasks(
    body: &str,
    note: &str,
    min_confidence: f32,
) -> Result<Vec<ProposedTask>, ChatError> {
    let json = strip_fences(body);
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
        // Clamped rather than rejected: a model reporting 1.2 is miscalibrated,
        // not lying about the task, and the floor is a backstop anyway — the
        // accept/dismiss step is the real precision mechanism.
        let confidence = raw.confidence.clamp(0.0, 1.0);
        if confidence < min_confidence {
            tracing::debug!("extraction: dropped {:?} below the floor", raw.text);
            continue;
        }
        kept.push(ProposedTask {
            text: raw.text.trim().to_string(),
            evidence: raw.evidence.trim().to_string(),
            confidence,
        });
    }
    Ok(kept)
}

/// Build the request for one note.
///
/// Asks for `json_object`, so the server's grammar constraint makes the answer
/// structurally valid by construction rather than by hope. `strip_fences`
/// stays anyway — the constraint is not honoured by every server version, and
/// the eval harness may point at one that is not.
pub fn build_request(cfg: &ExtractConfig, note: &str) -> ChatRequest {
    ChatRequest {
        model: cfg.model.clone(),
        messages: vec![
            Message::system(prompts::EXTRACT_SYSTEM),
            Message::user(note),
        ],
        response_format: Some(ResponseFormat::json_object()),
    }
}

/// Run one extraction pass.
pub async fn extract(
    base_url: &str,
    cfg: &ExtractConfig,
    note: &str,
    timeout: Duration,
) -> Result<Vec<ProposedTask>, ChatError> {
    let request = build_request(cfg, note);
    let body = chat::complete(base_url, &request, timeout).await?;
    parse_tasks(&body, note, cfg.min_confidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str =
        "I need to call the vet about Milo tomorrow. Sarah is sending the invoice on Tuesday.";

    fn one_task(evidence: &str, confidence: f32) -> String {
        format!(
            r#"{{"tasks":[{{"text":"Call the vet","evidence":"{evidence}","confidence":{confidence}}}]}}"#
        )
    }

    #[test]
    fn evidence_not_in_note_is_rejected_as_fabrication() {
        let body = one_task("I promised to rewire the whole house", 0.99);
        assert!(
            parse_tasks(&body, NOTE, 0.5).unwrap().is_empty(),
            "a task whose span is not in the note is invented, and confidence \
             says nothing about that — a fabricating model is confident"
        );
    }

    #[test]
    fn empty_evidence_is_not_grounding() {
        // Every string contains the empty string. Without an explicit guard a
        // model that simply omitted `evidence` would pass the one check that
        // does not depend on its judgment.
        let body = one_task("", 0.99);
        assert!(parse_tasks(&body, NOTE, 0.5).unwrap().is_empty());
    }

    #[test]
    fn an_empty_task_list_is_a_successful_answer_not_an_error() {
        // The common case. Most notes contain no tasks, and treating that as a
        // failure would light up a retry affordance on every ordinary note.
        assert_eq!(parse_tasks(r#"{"tasks":[]}"#, NOTE, 0.5).unwrap(), vec![]);
        assert_eq!(parse_tasks(r#"{}"#, NOTE, 0.5).unwrap(), vec![]);
    }

    #[test]
    fn fenced_json_is_tolerated() {
        let inner = one_task("I need to call the vet about Milo tomorrow", 0.9);
        for wrapped in [
            format!("```json\n{inner}\n```"),
            format!("```\n{inner}\n```"),
            format!("  ```json\n{inner}\n```  "),
        ] {
            let got = parse_tasks(&wrapped, NOTE, 0.5).unwrap();
            assert_eq!(got.len(), 1, "failed on: {wrapped}");
        }
    }

    #[test]
    fn a_task_at_exactly_the_floor_is_kept() {
        let body = one_task("I need to call the vet about Milo tomorrow", 0.5);
        assert_eq!(parse_tasks(&body, NOTE, 0.5).unwrap().len(), 1);

        let below = one_task("I need to call the vet about Milo tomorrow", 0.49);
        assert!(parse_tasks(&below, NOTE, 0.5).unwrap().is_empty());
    }

    #[test]
    fn the_floor_is_applied_here_and_not_left_to_the_ui() {
        // A row nobody is ever shown is not a labelled example. Letting it
        // through to be filtered at render time would put a decision nobody
        // made into tasks.json and poison the eval corpus.
        let body = one_task("I need to call the vet about Milo tomorrow", 0.2);
        assert!(parse_tasks(&body, NOTE, 0.5).unwrap().is_empty());
    }

    #[test]
    fn grounding_survives_the_whitespace_a_transcript_carries() {
        let note = "I need to  call the vet\nabout Milo tomorrow.";
        let body = one_task("I need to call the vet about milo tomorrow", 0.9);
        assert_eq!(
            parse_tasks(&body, note, 0.5).unwrap().len(),
            1,
            "a doubled space or a line break in the transcript must not read as a fabrication"
        );
    }

    #[test]
    fn an_out_of_range_confidence_is_clamped_not_fatal() {
        let body = one_task("I need to call the vet about Milo tomorrow", 4.2);
        let got = parse_tasks(&body, NOTE, 0.5).unwrap();
        assert_eq!(got[0].confidence, 1.0);
    }

    #[test]
    fn malformed_json_is_an_error_not_an_empty_list() {
        // These must be distinguishable: an empty list marks the stage Done,
        // a parse failure marks it Failed and offers a retry.
        assert!(matches!(
            parse_tasks("the model said something else entirely", NOTE, 0.5),
            Err(ChatError::Malformed(_))
        ));
    }

    #[test]
    fn the_extraction_request_asks_the_server_to_constrain_the_grammar() {
        let req = build_request(&ExtractConfig::default(), NOTE);
        assert_eq!(req.model, "gemma-4-E4B_q4_0-it");
        assert!(req.response_format.is_some());
        assert_eq!(req.messages[0].content, prompts::EXTRACT_SYSTEM);
        assert_eq!(req.messages[1].content, NOTE, "the note is sent as-is");
    }
}

//! Stage 1 — transcript cleanup with S1-mini.
//!
//! The model removes fillers, resolves false starts to whatever the speaker
//! landed on, applies punctuation and capitalization, and renders spoken
//! numbers, dates, times and currency in written form. It is a *normalizer*,
//! not a chat model: it cannot be asked to do anything else, which is exactly
//! why it cannot wander off and "improve" a note.
//!
//! Everything about the request shape that could be wrong lives in
//! `prompts.rs`, behind types. What is left here is the decision of what a
//! response *means*, which is the part with a genuine trap in it — see
//! [`resolve`].

use std::time::Duration;

use super::chat::{self, ChatError, ChatRequest, Message};
use super::prompts::{self, NoteContext, Structure, Styling};
use super::CleanupConfig;

/// What a completed cleanup pass actually asks the store to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cleaned {
    /// The model produced different text. This is the text to display.
    Rewritten(String),
    /// The model read the transcript and had nothing to change.
    ///
    /// A **successful** outcome, and a common one. Two different responses
    /// arrive here: an empty string, which is what filler-only speech
    /// correctly normalizes to, and a response identical to the input, which
    /// is what already-clean speech produces.
    NothingToChange,
}

/// Decide what a raw completion means for the note.
///
/// ⚠️ **An empty response is success, not failure.** Say "um, uh, so" into a
/// note and S1-mini correctly returns nothing at all — there was no content to
/// normalize. Treating that as an error would be merely wrong; treating it as
/// a rewrite would be destructive, because it would blank the only record of
/// what was said. The intuitive implementation does one or the other, so the
/// guard is written deliberately and pinned by a test.
///
/// `sent` is the text the request carried, so an unchanged response is
/// recognised rather than written back over itself and marked as a rewrite.
pub fn resolve(response: &str, sent: &str) -> Cleaned {
    let cleaned = response.trim();
    if cleaned.is_empty() || cleaned == sent.trim() {
        return Cleaned::NothingToChange;
    }
    Cleaned::Rewritten(cleaned.to_string())
}

/// Build the request for one transcript.
///
/// No `response_format`: the model answers in plain text, and constraining it
/// to JSON would be constraining it away from what it was trained to emit.
pub fn build_request(cfg: &CleanupConfig, transcript: &str) -> ChatRequest {
    let styling = Styling::from_config_name(&cfg.styling);
    let structure = Structure::from_config_name(&cfg.structure);
    let context = NoteContext::from_config_name(&cfg.context);

    ChatRequest {
        model: cfg.model.clone(),
        messages: vec![
            Message::system(prompts::CLEANUP_SYSTEM),
            Message::user(prompts::cleanup_user_message(
                styling, structure, context, transcript,
            )),
        ],
        response_format: None,
    }
}

/// Run one cleanup pass.
///
/// The caller keeps hold of the `transcript` it passed: applying the result
/// back to the note is a compare-and-swap against that exact text, because the
/// user may have typed into the note while the model was thinking.
pub async fn clean(
    base_url: &str,
    cfg: &CleanupConfig,
    transcript: &str,
    timeout: Duration,
) -> Result<Cleaned, ChatError> {
    let request = build_request(cfg, transcript);
    let response = chat::complete(base_url, &request, timeout).await?;
    Ok(resolve(&response, transcript))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_response_resolves_to_nothing_to_change() {
        assert_eq!(resolve("", "um uh hmm"), Cleaned::NothingToChange);
        assert_eq!(resolve("   \n ", "um uh hmm"), Cleaned::NothingToChange);
    }

    #[test]
    fn a_response_identical_to_the_input_is_not_a_rewrite() {
        // Already-clean speech. Reporting this as a rewrite would mark the
        // note modified and dirty the store for a change that did not happen.
        assert_eq!(
            resolve("Call the vet.\n", "Call the vet."),
            Cleaned::NothingToChange
        );
    }

    #[test]
    fn a_real_rewrite_comes_back_trimmed() {
        assert_eq!(
            resolve("  Call the vet about Milo.\n", "um so call the vet about milo"),
            Cleaned::Rewritten("Call the vet about Milo.".to_string())
        );
    }

    #[test]
    fn the_request_carries_the_model_card_system_prompt_and_a_control_line() {
        let cfg = CleanupConfig::default();
        let req = build_request(&cfg, "um so call the vet");

        assert_eq!(req.model, "s1-mini-q4_k_m");
        assert_eq!(req.messages.len(), 2, "system then user, nothing else");
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[0].content, prompts::CLEANUP_SYSTEM);

        let user = &req.messages[1].content;
        assert!(
            user.starts_with("[Styling: semi-formal] [Structure: lists] [Context: general]\n"),
            "the control line is the first line of the user message: {user}"
        );
        assert!(user.ends_with("um so call the vet"));
    }

    #[test]
    fn cleanup_asks_for_plain_text_not_json() {
        // s1-mini emits normalized prose. A json_object grammar constraint
        // would force it away from the only output it was trained to produce.
        let req = build_request(&CleanupConfig::default(), "hello");
        assert!(req.response_format.is_none());
    }

    #[test]
    fn a_garbage_config_value_still_sends_a_trained_control_line() {
        // Hand-edited config is the realistic source of this. Falling back to
        // the trained default is the only safe answer: sending
        // "[Styling: shakespearean]" is documented to garble the output, and
        // the server would answer 200 with the garbage.
        let cfg = CleanupConfig {
            styling: "shakespearean".into(),
            structure: "haiku".into(),
            context: "telegram".into(),
            ..CleanupConfig::default()
        };
        let req = build_request(&cfg, "hello");
        assert!(
            req.messages[1]
                .content
                .starts_with("[Styling: semi-formal] [Structure: lists] [Context: general]"),
            "got: {}",
            req.messages[1].content
        );
    }
}

//! Stage 1 — transcript cleanup with S1-mini: a normalizer (fillers, false
//! starts, punctuation, spoken numbers/dates), not a chat model. The request
//! shape lives in `prompts.rs` behind types; what is left here is what a
//! response *means* — see [`resolve`].

use std::time::Duration;

use super::chat::{self, ChatError, ChatRequest, Message};
use super::prompts::{self, NoteContext, Structure, Styling};
use super::CleanupConfig;

/// What a completed cleanup pass actually asks the store to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cleaned {
    Rewritten(String),
    /// Success, and common: filler-only speech normalizes to empty, and
    /// already-clean speech comes back identical.
    NothingToChange,
}

/// Decide what a raw completion means. Empty or unchanged is success, not
/// failure: filler-only speech normalizes to nothing, and writing that back
/// would blank the only record. `sent` is the request text, so unchanged input
/// is recognised, not rewritten.
pub fn resolve(response: &str, sent: &str) -> Cleaned {
    let cleaned = response.trim();
    if cleaned.is_empty() || cleaned == sent.trim() {
        return Cleaned::NothingToChange;
    }
    Cleaned::Rewritten(cleaned.to_string())
}

/// Build the request for one transcript. No `response_format`: the model
/// answers in plain text, not JSON.
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

/// Run one cleanup pass. The caller applies the result as a compare-and-swap
/// against `transcript`, which it retains: the user may type while the model
/// is thinking.
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
        // A rewrite here would dirty the store for a change that never happened.
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
        let req = build_request(&CleanupConfig::default(), "hello");
        assert!(req.response_format.is_none());
    }

    #[test]
    fn a_garbage_config_value_still_sends_a_trained_control_line() {
        // An out-of-set control line garbles output at HTTP 200, so unknown
        // values fall back to the trained default.
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

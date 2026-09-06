//! `POST /v1/chat/completions` against the standalone llama.cpp server.
//! The body carries model, messages and nothing else: sampling and the thinking
//! switch live server-side in `deploy/llama-models.ini`. Both models answer
//! HTTP 200 when misconfigured, so a test pins that Beamer sends no sampling
//! parameters.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::client::{failure_message, http_client};

pub fn chat_url(base_url: &str) -> String {
    format!("{}/v1/chat/completions", base_url.trim_end_matches('/'))
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system", content: content.into() }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user", content: content.into() }
    }
}

/// Grammar constraint requesting valid JSON. Not honoured by every server
/// version, so the extraction parser still tolerates fences.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

impl ResponseFormat {
    pub fn json_object() -> Self {
        Self { kind: "json_object" }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    /// Omitted when absent — `null` is not unconstrained to every server.

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// Why a completion produced no usable text. Not `anyhow::Error`: callers branch on these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatError {
    /// Server not answering (refused, DNS, timeout). Normal: the server is standalone.
    Unreachable(String),
    Http(u16),
    Malformed(String),
    /// `content` empty while `reasoning_content` is not: the server preset is
    /// missing its thinking switch. Status is 200 with valid JSON, so without
    /// this variant it surfaces as an inscrutable parse error.
    ThinkingEnabled,
}

impl ChatError {
    /// Whether retrying unchanged could succeed. Unreachable hosts, timeouts
    /// and 5xx are transient; 4xx (wrong model name), malformed bodies and a
    /// misconfigured server preset need a config or server change first. The
    /// pipeline still records both as `Failed` — only the backlog sweep
    /// treats them differently.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Unreachable(_) => true,
            Self::Http(code) => *code >= 500,
            Self::Malformed(_) | Self::ThinkingEnabled => false,
        }
    }
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(msg) => write!(f, "{msg}"),
            Self::Http(code) => write!(f, "Server returned HTTP {code}"),
            Self::Malformed(msg) => write!(f, "Unexpected response: {msg}"),
            Self::ThinkingEnabled => write!(
                f,
                "The model answered with reasoning only. Its server preset in \
                 llama-models.ini is missing its thinking switch."
            ),
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    message: ResponseMessage,
}

#[derive(Deserialize, Default)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    /// Never output; exists only to diagnose [`ChatError::ThinkingEnabled`].
    #[serde(default)]
    reasoning_content: Option<String>,
}

/// Pull the assistant's text out of a completion body. Split from the request
/// because `reqwest::Error` cannot be built in tests. Empty `Ok("")` is a
/// correct answer (filler-only speech); the caller decides what it means.
pub fn parse_completion(body: &str) -> Result<String, ChatError> {
    let parsed: ChatResponse =
        serde_json::from_str(body).map_err(|e| ChatError::Malformed(e.to_string()))?;

    let Some(choice) = parsed.choices.into_iter().next() else {
        return Err(ChatError::Malformed("no choices in response".into()));
    };

    let content = choice.message.content.unwrap_or_default();
    let reasoning = choice.message.reasoning_content.unwrap_or_default();

    if content.is_empty() && !reasoning.is_empty() {
        return Err(ChatError::ThinkingEnabled);
    }

    Ok(content)
}

/// Placeholder-token opener, spelled out: this module cannot reach the note
/// block helper, and a test there pins the literal.
const NOTE_TOKEN_MARKER: &str = "[[beamer:";

/// Send one completion and return the assistant's text. Does not probe
/// `GET /v1/models` first: a status read resets the per-model idle clock and
/// would pin the extraction model in VRAM with no symptom.
pub async fn complete(
    base_url: &str,
    request: &ChatRequest,
    timeout: Duration,
) -> Result<String, ChatError> {
    // A placeholder token here makes the model answer garbage at HTTP 200 —
    // nothing downstream can catch it, so this warns rather than just logging.

    if let Some(bad) = request.messages.iter().find(|m| m.content.contains(NOTE_TOKEN_MARKER)) {
        tracing::warn!(
            "chat request to {} carries a note placeholder token in its {} message — \
             the reply will be garbage and the server will still answer 200",
            request.model, bad.role
        );
    }
    tracing::debug!(
        "chat request: model={} messages={} chars={}",
        request.model,
        request.messages.len(),
        request.messages.iter().map(|m| m.content.len()).sum::<usize>()
    );

    let response = http_client()
        .post(chat_url(base_url))
        .json(request)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| ChatError::Unreachable(failure_message(e.is_timeout(), e.is_connect(), None)))?;

    if !response.status().is_success() {
        return Err(ChatError::Http(response.status().as_u16()));
    }

    let body = response
        .text()
        .await
        .map_err(|e| ChatError::Unreachable(failure_message(e.is_timeout(), false, None)))?;

    parse_completion(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> ChatRequest {
        ChatRequest {
            model: "s1-mini-q4_k_m".into(),
            messages: vec![Message::system("be a normalizer"), Message::user("um so hello")],
            response_format: None,
        }
    }

    #[test]
    fn the_request_body_carries_no_sampling_parameters() {
        let json = serde_json::to_string(&sample_request()).unwrap();
        for forbidden in [
            "temperature",
            "temp",
            "top_k",
            "top_p",
            "enable_thinking",
            "chat_template_kwargs",
            "reasoning_budget",
            "max_tokens",
        ] {
            assert!(
                !json.contains(forbidden),
                "`{forbidden}` is owned by deploy/llama-models.ini. Sending it from \
                 Beamer gives one setting two owners, and the disagreement is silent \
                 — the server answers 200 either way. Body was: {json}"
            );
        }
        assert!(json.contains("\"model\""));
        assert!(json.contains("\"messages\""));
    }

    #[test]
    fn an_absent_response_format_is_omitted_rather_than_null() {
        let json = serde_json::to_string(&sample_request()).unwrap();
        assert!(
            !json.contains("response_format"),
            "a null response_format is not the same as no response_format to every \
             server version: {json}"
        );

        let mut constrained = sample_request();
        constrained.response_format = Some(ResponseFormat::json_object());
        let json = serde_json::to_string(&constrained).unwrap();
        assert!(json.contains(r#""response_format":{"type":"json_object"}"#), "{json}");
    }

    #[test]
    fn empty_content_with_reasoning_content_is_diagnosed_as_thinking_enabled() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"",
                       "reasoning_content":"The user wants me to think about this."}}]}"#;
        assert_eq!(parse_completion(body), Err(ChatError::ThinkingEnabled));
    }

    #[test]
    fn an_empty_completion_with_no_reasoning_is_a_successful_empty_answer() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":""}}]}"#;
        assert_eq!(parse_completion(body), Ok(String::new()));
    }

    #[test]
    fn reasoning_alongside_real_content_is_not_an_error() {
        let body = r#"{"choices":[{"message":{"content":"Hello.","reasoning_content":"hmm"}}]}"#;
        assert_eq!(parse_completion(body), Ok("Hello.".to_string()));
    }

    #[test]
    fn a_response_with_no_choices_is_malformed_not_empty() {
        // Ok("") here would mark a broken pass as Done.
        assert!(matches!(
            parse_completion(r#"{"choices":[]}"#),
            Err(ChatError::Malformed(_))
        ));
        assert!(matches!(parse_completion("not json"), Err(ChatError::Malformed(_))));
    }

    #[test]
    fn only_transient_errors_are_retryable() {
        assert!(ChatError::Unreachable("connection refused".into()).is_retryable());
        assert!(ChatError::Http(500).is_retryable());
        assert!(ChatError::Http(503).is_retryable());
        assert!(!ChatError::Http(400).is_retryable());
        assert!(!ChatError::Http(404).is_retryable());
        assert!(!ChatError::Malformed("no choices".into()).is_retryable());
        assert!(!ChatError::ThinkingEnabled.is_retryable());
    }

    #[test]
    fn chat_url_tolerates_a_trailing_slash() {
        assert_eq!(
            chat_url("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            chat_url("http://127.0.0.1:8080///"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
    }
}

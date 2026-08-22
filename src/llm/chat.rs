//! `POST /v1/chat/completions` against the standalone llama.cpp server.
//!
//! Mirrors `client.rs`: plain async `reqwest` on the runtime dioxus-desktop
//! owns, sharing its connection pool rather than opening a second one.
//!
//! **The request body carries the model, the messages and nothing else.**
//! Sampling parameters and the thinking switch both models require are set
//! server-side in `deploy/llama-models.ini`. Re-sending them from here would
//! create two owners of one setting, and the failure mode is silent: both
//! models answer HTTP 200 with a plausible body when misconfigured, so a
//! Beamer-side `temperature` that disagreed with the preset would degrade
//! output with nothing to catch. A test pins their absence.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::client::{failure_message, http_client};

/// `POST {base_url}/v1/chat/completions`, tolerating a trailing slash on the
/// configured URL for the same reason `models_url` does.
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

/// llama.cpp's grammar constraint. Requesting it makes the server emit
/// structurally valid JSON by construction rather than by hope — but not every
/// server version honours it, which is why the extraction parser still
/// tolerates fences.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

impl ResponseFormat {
    /// Used by the extraction stage; cleanup deliberately sends none.
    #[allow(dead_code)]
    pub fn json_object() -> Self {
        Self { kind: "json_object" }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    /// Omitted entirely when absent — a `null` here is not the same as an
    /// unconstrained request to every server version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// Why a completion did not produce usable text.
///
/// Deliberately not `anyhow::Error`: the caller branches on these, and
/// `ThinkingEnabled` in particular has a specific remedy to name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatError {
    /// Connection refused, DNS, timeout — the server is not answering. This is
    /// a normal state, not an error condition: the server is standalone and
    /// may simply not be running.
    Unreachable(String),
    /// A response arrived, with a status that was not 2xx.
    Http(u16),
    /// A 2xx body that was not the shape we expect.
    Malformed(String),
    /// `content` was empty while `reasoning_content` was not.
    ///
    /// This is the exact misconfiguration both models fall into when the
    /// server preset is missing its thinking switch, and it is worth its own
    /// variant because it otherwise surfaces as an inscrutable parse error.
    /// Neither model reports it: the HTTP status is 200 and the body is valid
    /// JSON. Only the empty/non-empty pairing gives it away.
    ThinkingEnabled,
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
    /// Present only when the model reasoned. Never used as output — it exists
    /// here solely to diagnose [`ChatError::ThinkingEnabled`].
    #[serde(default)]
    reasoning_content: Option<String>,
}

/// Pull the assistant's text out of a completion body.
///
/// Split from the request for the same reason `client.rs` splits
/// `failure_message`: a `reqwest::Error` cannot be constructed in a test, and
/// this is the part that can actually be wrong.
///
/// An empty string is returned as `Ok("")`, not an error. For cleanup that is
/// a *correct* answer — filler-only speech normalizes to nothing — so the
/// decision of what empty means belongs to the caller, not here.
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

/// Send one completion and return the assistant's text.
///
/// ⚠️ Does **not** probe `GET /v1/models` first. A status read resets the
/// server's per-model idle clock, so probing before every request would pin
/// the extraction model in VRAM permanently with no error and no symptom.
/// Connection-refused is fast and well classified; just make the call.
pub async fn complete(
    base_url: &str,
    request: &ChatRequest,
    timeout: Duration,
) -> Result<String, ChatError> {
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
        // Verbatim shape of what llama-server returns for Gemma 4 when its
        // preset is missing chat-template-kwargs: HTTP 200, valid JSON, the
        // answer sitting in the wrong field. Without this variant the caller
        // sees "cleaned to nothing" and quietly blanks a working feature.
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"",
                       "reasoning_content":"The user wants me to think about this."}}]}"#;
        assert_eq!(parse_completion(body), Err(ChatError::ThinkingEnabled));
    }

    #[test]
    fn an_empty_completion_with_no_reasoning_is_a_successful_empty_answer() {
        // s1-mini genuinely returns "" for filler-only speech. That is the
        // model working, not failing, so it must not be an error here.
        let body = r#"{"choices":[{"message":{"role":"assistant","content":""}}]}"#;
        assert_eq!(parse_completion(body), Ok(String::new()));
    }

    #[test]
    fn reasoning_alongside_real_content_is_not_an_error() {
        // Thinking that still produced an answer is wasteful, not broken. The
        // diagnosis is specifically the *empty content* pairing.
        let body = r#"{"choices":[{"message":{"content":"Hello.","reasoning_content":"hmm"}}]}"#;
        assert_eq!(parse_completion(body), Ok("Hello.".to_string()));
    }

    #[test]
    fn a_response_with_no_choices_is_malformed_not_empty() {
        // Returning Ok("") here would be indistinguishable from a correct
        // empty cleanup, and would mark a broken pass as Done.
        assert!(matches!(
            parse_completion(r#"{"choices":[]}"#),
            Err(ChatError::Malformed(_))
        ));
        assert!(matches!(parse_completion("not json"), Err(ChatError::Malformed(_))));
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

use std::fmt::Write as _;

use anyhow::{Context, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tungstenite::Message;

use crate::audio::warn_channel_full;

use super::keyterms;
use super::{
    RealtimeSession, TranscriptEvent, TranscriptKind, AUDIO_CHANNEL_CAPACITY,
    TRANSCRIPT_CHANNEL_CAPACITY,
};

/// Message types carrying transcript/session state rather than a failure.
const NON_ERROR_MESSAGE_TYPES: [&str; 7] = [
    "session_started",
    "partial_transcript",
    "committed_transcript",
    "committed_transcript_with_timestamps",
    "committed_transcript_entities",
    "final_transcript",
    "final_transcript_with_timestamps",
];

/// Build the realtime WebSocket URL. Keyterms are repeated, percent-encoded
/// `keyterms` query parameters — there is no session config message, so
/// everything must be settled before the handshake.
fn build_realtime_url(language: &str, terms: &[String], no_verbatim: bool) -> String {
    let mut url = format!(
        "wss://api.elevenlabs.io/v1/speech-to-text/realtime\
         ?model_id=scribe_v2_realtime\
         &language_code={}\
         &audio_format=pcm_16000\
         &commit_strategy=manual",
        language
    );
    if no_verbatim {
        url.push_str("&no_verbatim=true");
    }
    for term in terms {
        let _ = write!(url, "&keyterms={}", keyterms::encode_query_value(term));
    }
    url
}

/// Render a server message as an error string, or `None` if it isn't one.
/// Matches the shape, not an enumerated list: any non-transcript message
/// carrying an error string is a failure worth surfacing.
fn describe_error(message_type: &str, parsed: &serde_json::Value) -> Option<String> {
    if NON_ERROR_MESSAGE_TYPES.contains(&message_type) {
        return None;
    }
    let detail = parsed["error"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or("");
    let code = parsed["code"].as_str().unwrap_or("");
    match (detail.is_empty(), code.is_empty()) {
        // Nothing usable in the payload: not an error, just an unknown message.
        (true, true) => None,
        (true, false) => Some(format!("{message_type} ({code})")),
        (false, true) => Some(format!("{message_type}: {detail}")),
        (false, false) => Some(format!("{message_type} ({code}): {detail}")),
    }
}

/// Build an `input_audio_chunk` frame into `out`, reusing its allocation.
/// `commit` is `true` only for the end-of-audio convention (empty chunk).
/// Base64 needs no JSON escaping, so `b64` is written directly.
fn build_audio_chunk_frame(b64: &str, commit: bool, out: &mut String) {
    out.clear();
    let _ = write!(
        out,
        r#"{{"message_type":"input_audio_chunk","audio_base_64":"{b64}","commit":{commit},"sample_rate":16000}}"#
    );
}

/// Open a WebSocket to the ElevenLabs Scribe v2 realtime endpoint with
/// `commit_strategy=manual`, so the caller controls finalization. Pass empty
/// `vocab` for sessions whose transcript is discarded (e.g. warmup): keyterms
/// carry a surcharge and would be paid for nothing.
pub async fn start_realtime_session(
    api_key: &str,
    language: &str,
    vocab: &[String],
    no_verbatim: bool,
) -> Result<RealtimeSession> {
    let terms = keyterms::sanitize(
        vocab,
        keyterms::REALTIME_MAX_TERMS,
        keyterms::REALTIME_MAX_CHARS,
    );
    if !terms.is_empty() {
        tracing::debug!("ElevenLabs realtime: sending {} keyterms", terms.len());
    }
    let url = build_realtime_url(&keyterms::encode_query_value(language), &terms, no_verbatim);

    let request = tungstenite::http::Request::builder()
        .uri(&url)
        .header("xi-api-key", api_key)
        .header("Host", "api.elevenlabs.io")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .context("Failed to build WebSocket request")?;

    // Bounded: `connect_async` imposes no timeout, and a stalled handshake
    // would strand the recording loop (hotkey owner) and the startup warmup.
    let (ws_stream, _) = tokio::time::timeout(
        super::WS_CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "ElevenLabs did not answer the WebSocket handshake within {}s",
            super::WS_CONNECT_TIMEOUT.as_secs()
        )
    })?
    .context("WebSocket connection failed")?;

    let (mut write, mut read) = ws_stream.split();

    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(AUDIO_CHANNEL_CAPACITY);
    let (transcript_tx, transcript_rx) =
        mpsc::channel::<TranscriptEvent>(TRANSCRIPT_CHANNEL_CAPACITY);

    let sender = tokio::spawn(async move {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut b64_buf = String::new();
        let mut frame_buf = String::new();
        while let Some(chunk) = audio_rx.recv().await {
            if chunk.is_empty() {
                // Convention: empty Vec triggers manual commit
                build_audio_chunk_frame("", true, &mut frame_buf);
            } else {
                b64_buf.clear();
                engine.encode_string(&chunk, &mut b64_buf);
                build_audio_chunk_frame(&b64_buf, false, &mut frame_buf);
            }
            // Bounded: a server that stops reading must end the session, not
            // back-press this send until the commit never goes out.
            let sent = tokio::time::timeout(
                super::WS_SEND_TIMEOUT,
                write.send(Message::Text(frame_buf.as_str().into())),
            )
            .await;
            match sent {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    tracing::warn!("ElevenLabs realtime send timed out — ending session");
                    break;
                }
            }
        }
        let _ = write.close().await;
    });

    let reader = tokio::spawn(async move {
        let mut dropped_transcripts: u64 = 0;
        let mut send_event = |ev: TranscriptEvent| {
            // Warn only on `Full`. `Closed` is normal session teardown, which
            // every session reaches on its trailing "WebSocket closed" event.
            match transcript_tx.try_send(ev) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn_channel_full(&mut dropped_transcripts, "ElevenLabs transcript");
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        };

        loop {
            let msg = match read.next().await {
                Some(Ok(msg)) => msg,
                // A reset, protocol error or idle timeout must surface as an
                // error, not dissolve into the trailing "WebSocket closed" info.
                Some(Err(e)) => {
                    let err = format!("WebSocket error: {}", e);
                    tracing::error!("ElevenLabs realtime transport error: {}", err);
                    send_event(TranscriptEvent {
                        text: String::new(),
                        kind: TranscriptKind::Error(err),
                    });
                    break;
                }
                None => break,
            };
            if let Message::Text(text) = msg {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    match parsed["message_type"].as_str() {
                        Some("session_started") => {
                            let sid = parsed["session_id"]
                                .as_str()
                                .unwrap_or("?")
                                .to_string();
                            tracing::info!("ElevenLabs session started: {}", sid);
                            send_event(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::SessionStarted(sid),
                            });
                        }
                        Some("partial_transcript") => {
                            let t = parsed["text"].as_str().unwrap_or("").to_string();
                            send_event(TranscriptEvent {
                                text: t,
                                kind: TranscriptKind::Partial,
                            });
                        }
                        Some("committed_transcript") => {
                            let t = parsed["text"].as_str().unwrap_or("").to_string();
                            send_event(TranscriptEvent {
                                text: t,
                                kind: TranscriptKind::Final,
                            });
                        }
                        Some(other) => match describe_error(other, &parsed) {
                            Some(err) => {
                                tracing::error!("ElevenLabs realtime error: {}", err);
                                send_event(TranscriptEvent {
                                    text: String::new(),
                                    kind: TranscriptKind::Error(err),
                                });
                            }
                            None => {
                                tracing::debug!("Unhandled WS message type: {:?}", other);
                            }
                        },
                        None => {
                            tracing::debug!("WS message without a message_type: {}", text);
                        }
                    }
                }
            }
        }
        send_event(TranscriptEvent {
            text: String::new(),
            kind: TranscriptKind::Info("WebSocket closed".to_string()),
        });
    });

    Ok(RealtimeSession::new(audio_tx, transcript_rx, vec![sender, reader]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// `commit_strategy=manual` is what lets the hotkey, not the server's VAD,
    /// decide when a transcript is final.
    #[test]
    fn a_session_without_keyterms_builds_the_plain_url() {
        let url = build_realtime_url("en", &[], false);
        assert_eq!(
            url,
            "wss://api.elevenlabs.io/v1/speech-to-text/realtime\
             ?model_id=scribe_v2_realtime\
             &language_code=en\
             &audio_format=pcm_16000\
             &commit_strategy=manual"
        );
    }

    /// One repeated `keyterms` parameter per term.
    #[test]
    fn each_keyterm_becomes_its_own_query_parameter() {
        let url = build_realtime_url("en", &terms(&["Beamer", "Dioxus"]), false);
        assert!(url.ends_with("&keyterms=Beamer&keyterms=Dioxus"), "{url}");
    }

    /// A raw space would break the upgrade request rather than the term.
    #[test]
    fn keyterms_are_percent_encoded_in_the_url() {
        let url = build_realtime_url("en", &terms(&["Deploy Purple", "R&D"]), false);
        assert!(url.contains("&keyterms=Deploy%20Purple"), "{url}");
        assert!(url.contains("&keyterms=R%26D"), "{url}");
    }

    /// The parameter is only sent when switched on.
    #[test]
    fn no_verbatim_appears_only_when_enabled() {
        assert!(!build_realtime_url("en", &[], false).contains("no_verbatim"));
        assert!(build_realtime_url("en", &[], true).contains("&no_verbatim=true"));
    }

    #[test]
    fn the_language_code_is_the_one_it_was_given() {
        assert!(build_realtime_url("de", &[], false).contains("&language_code=de"));
    }

    #[test]
    fn transcript_and_session_messages_are_not_errors() {
        for kind in NON_ERROR_MESSAGE_TYPES {
            let msg = serde_json::json!({ "message_type": kind, "text": "hello" });
            assert_eq!(describe_error(kind, &msg), None, "{kind}");
        }
    }

    /// A `quota_exceeded` failure must surface with its message, not vanish.
    #[test]
    fn a_quota_error_is_surfaced_with_its_message() {
        let msg = serde_json::json!({
            "message_type": "quota_exceeded",
            "error": "You have exceeded your character quota."
        });
        assert_eq!(
            describe_error("quota_exceeded", &msg).as_deref(),
            Some("quota_exceeded: You have exceeded your character quota.")
        );
    }

    /// Shape matching covers error types that do not exist yet too.
    #[test]
    fn every_documented_error_type_is_recognised() {
        for kind in [
            "error",
            "auth_error",
            "quota_exceeded",
            "commit_throttled",
            "unaccepted_terms",
            "rate_limited",
            "queue_overflow",
            "resource_exhausted",
            "session_time_limit_exceeded",
            "input_error",
            "invalid_request",
            "chunk_size_exceeded",
            "insufficient_audio_activity",
            "transcriber_error",
            "some_error_type_invented_next_year",
        ] {
            let msg = serde_json::json!({ "message_type": kind, "error": "boom" });
            assert_eq!(
                describe_error(kind, &msg).as_deref(),
                Some(format!("{kind}: boom").as_str()),
                "{kind}"
            );
        }
    }

    /// Older `code`/`message` payload shape still reads sensibly.
    #[test]
    fn a_code_and_message_payload_still_reads_sensibly() {
        let msg = serde_json::json!({
            "message_type": "input_error",
            "code": "bad_audio",
            "message": "unsupported sample rate"
        });
        assert_eq!(
            describe_error("input_error", &msg).as_deref(),
            Some("input_error (bad_audio): unsupported sample rate")
        );
    }

    /// An unknown message with nothing error-shaped in it is ignored.
    #[test]
    fn an_unknown_message_carrying_no_error_is_ignored() {
        let msg = serde_json::json!({ "message_type": "vad_score", "score": 0.4 });
        assert_eq!(describe_error("vad_score", &msg), None);
    }

    /// The hand-built frame must match the `json!` construction for a sample chunk.
    #[test]
    fn audio_chunk_frame_matches_json_macro_for_sample_chunk() {
        let chunk: Vec<u8> = vec![0, 1, 2, 3, 250, 251, 252, 253, 254, 255];
        let engine = base64::engine::general_purpose::STANDARD;
        let b64 = engine.encode(&chunk);

        let mut out = String::new();
        build_audio_chunk_frame(&b64, false, &mut out);
        let actual: serde_json::Value = serde_json::from_str(&out).expect("valid json");

        let expected = serde_json::json!({
            "message_type": "input_audio_chunk",
            "audio_base_64": engine.encode(&chunk),
            "commit": false,
            "sample_rate": 16000
        });

        assert_eq!(actual, expected);
    }

    /// The end-of-audio (empty chunk) frame must match the `json!` construction.
    #[test]
    fn audio_chunk_frame_matches_json_macro_for_empty_chunk() {
        let mut out = String::new();
        build_audio_chunk_frame("", true, &mut out);
        let actual: serde_json::Value = serde_json::from_str(&out).expect("valid json");

        let expected = serde_json::json!({
            "message_type": "input_audio_chunk",
            "audio_base_64": "",
            "commit": true,
            "sample_rate": 16000
        });

        assert_eq!(actual, expected);
    }

    /// The reused buffer must not leak stale content between calls.
    #[test]
    fn build_audio_chunk_frame_clears_stale_buffer_contents() {
        let mut out = String::from("leftover garbage from a previous frame");
        build_audio_chunk_frame("QQ==", false, &mut out);
        let actual: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(
            actual,
            serde_json::json!({
                "message_type": "input_audio_chunk",
                "audio_base_64": "QQ==",
                "commit": false,
                "sample_rate": 16000
            })
        );
    }
}

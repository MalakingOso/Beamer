use std::fmt::Write as _;

use anyhow::{Context, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tungstenite::Message;

use crate::audio::warn_channel_full;

use super::{
    RealtimeSession, TranscriptEvent, TranscriptKind, AUDIO_CHANNEL_CAPACITY,
    TRANSCRIPT_CHANNEL_CAPACITY,
};

/// Build the `input_audio.append` frame for a non-empty chunk into `out`,
/// reusing its allocation. Base64 needs no JSON escaping.
fn build_audio_frame(b64: &str, out: &mut String) {
    out.clear();
    let _ = write!(out, r#"{{"type":"input_audio.append","audio":"{b64}"}}"#);
}

/// Build the `input_audio.end` frame (end-of-audio convention: empty chunk).
fn build_end_of_audio_frame(out: &mut String) {
    out.clear();
    out.push_str(r#"{"type":"input_audio.end"}"#);
}

/// Render a server `error` message as a string. Handles the object shape
/// (`{"error": {"message": ...}}`), the bare-string shape
/// (`{"error": "..."}`), and a top-level `message` field; anything else
/// falls back to the raw frame so nothing is ever swallowed silently.
fn describe_error(parsed: &serde_json::Value, raw_frame: &str) -> String {
    parsed["error"]["message"]
        .as_str()
        .or_else(|| parsed["error"].as_str())
        .or_else(|| parsed["message"].as_str())
        .unwrap_or(raw_frame)
        .to_string()
}

/// Open a WebSocket to the Mistral Voxtral Mini realtime endpoint. Requires an
/// initial `session.update` declaring pcm_s16le @ 16 kHz; language is auto-detected.
pub async fn start_realtime_session(api_key: &str) -> Result<RealtimeSession> {
    let url = "wss://api.mistral.ai/v1/audio/transcriptions/realtime\
               ?model=voxtral-mini-transcribe-realtime-2602";

    let request = tungstenite::http::Request::builder()
        .uri(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Host", "api.mistral.ai")
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
            "Voxtral did not answer the WebSocket handshake within {}s",
            super::WS_CONNECT_TIMEOUT.as_secs()
        )
    })?
    .context("WebSocket connection failed")?;

    let (mut write, mut read) = ws_stream.split();

    let session_config = serde_json::json!({
        "type": "session.update",
        "session": {
            "audio_format": {
                "encoding": "pcm_s16le",
                "sample_rate": 16000
            }
        }
    });
    // Bounded like every other send: a post-handshake stall must fail the
    // session here, not hang `start_realtime_session` indefinitely.
    tokio::time::timeout(
        super::WS_SEND_TIMEOUT,
        write.send(Message::Text(session_config.to_string().into())),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Timed out sending Voxtral session config"))?
    .context("Failed to send session config")?;

    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(AUDIO_CHANNEL_CAPACITY);
    let (transcript_tx, transcript_rx) =
        mpsc::channel::<TranscriptEvent>(TRANSCRIPT_CHANNEL_CAPACITY);

    let sender = tokio::spawn(async move {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut b64_buf = String::new();
        let mut frame_buf = String::new();
        while let Some(chunk) = audio_rx.recv().await {
            if chunk.is_empty() {
                build_end_of_audio_frame(&mut frame_buf);
            } else {
                b64_buf.clear();
                engine.encode_string(&chunk, &mut b64_buf);
                build_audio_frame(&b64_buf, &mut frame_buf);
            }
            let sent = tokio::time::timeout(
                super::WS_SEND_TIMEOUT,
                write.send(Message::Text(frame_buf.as_str().into())),
            )
            .await;
            match sent {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    tracing::warn!("Voxtral realtime send timed out — ending session");
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
                    warn_channel_full(&mut dropped_transcripts, "Voxtral transcript");
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
                    tracing::error!("Voxtral realtime transport error: {}", err);
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
                    match parsed["type"].as_str() {
                        Some("session.created") => {
                            let rid = parsed["session"]["request_id"]
                                .as_str()
                                .unwrap_or("?")
                                .to_string();
                            tracing::info!("Voxtral session created: {}", rid);
                            send_event(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::SessionStarted(rid),
                            });
                        }
                        Some("session.updated") => {
                            tracing::debug!("Voxtral session config updated");
                        }
                        Some("transcription.text.delta") => {
                            let t = parsed["text"].as_str().unwrap_or("").to_string();
                            send_event(TranscriptEvent {
                                text: t,
                                kind: TranscriptKind::Partial,
                            });
                        }
                        Some("transcription.done") => {
                            let t = parsed["text"].as_str().unwrap_or("").to_string();
                            send_event(TranscriptEvent {
                                text: t,
                                kind: TranscriptKind::Final,
                            });
                        }
                        Some("transcription.language") => {
                            let lang = parsed["language"].as_str().unwrap_or("?");
                            tracing::info!("Voxtral detected language: {}", lang);
                            send_event(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::Info(format!("Language: {}", lang)),
                            });
                        }
                        Some("transcription.segment") => {
                            tracing::debug!("Voxtral segment: {}", text);
                        }
                        Some("error") => {
                            let message = describe_error(&parsed, &text);
                            tracing::error!("Voxtral error: {}", message);
                            send_event(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::Error(message),
                            });
                        }
                        other => {
                            tracing::debug!("Unknown Voxtral message type: {:?}", other);
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

    /// The hand-built frame must match the `json!` construction for a sample chunk.
    #[test]
    fn audio_frame_matches_json_macro_for_sample_chunk() {
        let chunk: Vec<u8> = vec![0, 1, 2, 3, 250, 251, 252, 253, 254, 255];
        let engine = base64::engine::general_purpose::STANDARD;
        let b64 = engine.encode(&chunk);

        let mut out = String::new();
        build_audio_frame(&b64, &mut out);
        let actual: serde_json::Value = serde_json::from_str(&out).expect("valid json");

        let expected = serde_json::json!({
            "type": "input_audio.append",
            "audio": engine.encode(&chunk),
        });

        assert_eq!(actual, expected);
    }

    /// Every known error shape yields the message, never the raw frame.
    #[test]
    fn error_shapes_all_resolve_to_the_message() {
        let object = serde_json::json!({"type": "error", "error": {"message": "bad audio"}});
        assert_eq!(describe_error(&object, "RAW"), "bad audio");
        let string = serde_json::json!({"type": "error", "error": "boom"});
        assert_eq!(describe_error(&string, "RAW"), "boom");
        let top = serde_json::json!({"type": "error", "message": "top level"});
        assert_eq!(describe_error(&top, "RAW"), "top level");
        let unknown = serde_json::json!({"type": "error", "code": 7});
        assert_eq!(describe_error(&unknown, "RAW"), "RAW");
    }

    /// The end-of-audio (empty chunk) frame must match the `json!` construction.
    #[test]
    fn end_of_audio_frame_matches_json_macro_for_empty_chunk() {
        let mut out = String::new();
        build_end_of_audio_frame(&mut out);
        let actual: serde_json::Value = serde_json::from_str(&out).expect("valid json");

        let expected = serde_json::json!({"type": "input_audio.end"});

        assert_eq!(actual, expected);
    }

    /// The reused buffer must not leak stale content between calls.
    #[test]
    fn build_audio_frame_clears_stale_buffer_contents() {
        let mut out = String::from("leftover garbage from a previous frame");
        build_audio_frame("QQ==", &mut out);
        let actual: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(
            actual,
            serde_json::json!({"type": "input_audio.append", "audio": "QQ=="})
        );
    }
}

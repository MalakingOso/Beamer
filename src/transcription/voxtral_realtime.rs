use std::fmt::Write as _;

use anyhow::{Context, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tungstenite::Message;

use super::{RealtimeSession, TranscriptEvent, TranscriptKind};

/// Build the `input_audio.append` frame for a non-empty chunk into `out`,
/// reusing its allocation instead of building a fresh `serde_json::Value` +
/// `String` per frame.
///
/// Base64 output only ever contains `[A-Za-z0-9+/=]`, none of which require
/// JSON string escaping, so `b64` is safe to write directly into the
/// manually built JSON text below.
fn build_audio_frame(b64: &str, out: &mut String) {
    out.clear();
    let _ = write!(out, r#"{{"type":"input_audio.append","audio":"{b64}"}}"#);
}

/// Build the `input_audio.end` frame (the end-of-audio convention: an empty
/// chunk from the mic pipeline) into `out`.
fn build_end_of_audio_frame(out: &mut String) {
    out.clear();
    out.push_str(r#"{"type":"input_audio.end"}"#);
}

/// Open a WebSocket to the Mistral Voxtral Mini realtime transcription endpoint.
/// Requires an initial `session.update` message to configure audio format (pcm_s16le @ 16 kHz).
/// Language is auto-detected by the model.
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

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .context("WebSocket connection failed")?;

    let (mut write, mut read) = ws_stream.split();

    // Voxtral requires explicit audio format declaration before streaming
    let session_config = serde_json::json!({
        "type": "session.update",
        "session": {
            "audio_format": {
                "encoding": "pcm_s16le",
                "sample_rate": 16000
            }
        }
    });
    write
        .send(Message::Text(session_config.to_string().into()))
        .await
        .context("Failed to send session config")?;

    let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<TranscriptEvent>();

    // Audio sender: encodes PCM → base64 JSON and streams to the WebSocket
    tokio::spawn(async move {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut b64_buf = String::new();
        let mut frame_buf = String::new();
        while let Some(chunk) = audio_rx.recv().await {
            if chunk.is_empty() {
                // Convention: empty Vec signals end-of-audio
                build_end_of_audio_frame(&mut frame_buf);
                let _ = write.send(Message::Text(frame_buf.as_str().into())).await;
                continue;
            }
            b64_buf.clear();
            engine.encode_string(&chunk, &mut b64_buf);
            build_audio_frame(&b64_buf, &mut frame_buf);
            if write
                .send(Message::Text(frame_buf.as_str().into()))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = write.close().await;
    });

    // Transcript receiver: parses JSON messages into TranscriptEvents
    tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            if let Message::Text(text) = msg {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    match parsed["type"].as_str() {
                        Some("session.created") => {
                            let rid = parsed["session"]["request_id"]
                                .as_str()
                                .unwrap_or("?")
                                .to_string();
                            tracing::info!("Voxtral session created: {}", rid);
                            let _ = transcript_tx.send(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::SessionStarted(rid),
                            });
                        }
                        Some("session.updated") => {
                            tracing::debug!("Voxtral session config updated");
                        }
                        Some("transcription.text.delta") => {
                            let t = parsed["text"].as_str().unwrap_or("").to_string();
                            let _ = transcript_tx.send(TranscriptEvent {
                                text: t,
                                kind: TranscriptKind::Partial,
                            });
                        }
                        Some("transcription.done") => {
                            let t = parsed["text"].as_str().unwrap_or("").to_string();
                            let _ = transcript_tx.send(TranscriptEvent {
                                text: t,
                                kind: TranscriptKind::Final,
                            });
                        }
                        Some("transcription.language") => {
                            let lang = parsed["language"].as_str().unwrap_or("?");
                            tracing::info!("Voxtral detected language: {}", lang);
                            let _ = transcript_tx.send(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::Info(format!("Language: {}", lang)),
                            });
                        }
                        Some("transcription.segment") => {
                            tracing::debug!("Voxtral segment: {}", text);
                        }
                        Some("error") => {
                            let fallback = text.to_string();
                            let message = parsed["error"]["message"]
                                .as_str()
                                .unwrap_or(&fallback);
                            tracing::error!("Voxtral error: {}", message);
                            let _ = transcript_tx.send(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::Error(message.to_string()),
                            });
                        }
                        other => {
                            tracing::debug!("Unknown Voxtral message type: {:?}", other);
                        }
                    }
                }
            }
        }
        let _ = transcript_tx.send(TranscriptEvent {
            text: String::new(),
            kind: TranscriptKind::Info("WebSocket closed".to_string()),
        });
    });

    Ok(RealtimeSession {
        audio_tx,
        transcript_rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manually built `input_audio.append` frame must be structurally
    /// identical to what the old `json!{...}` + `.to_string()` construction
    /// produced, for a representative non-empty chunk.
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

    /// The end-of-audio frame (empty chunk convention) must remain
    /// byte-semantically identical to the old `json!{...}` construction.
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

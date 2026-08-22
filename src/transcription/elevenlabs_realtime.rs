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

/// Build an `input_audio_chunk` frame into `out`, reusing its allocation
/// instead of building a fresh `serde_json::Value` + `String` per frame.
/// `commit` should be `true` only for the end-of-audio convention (an empty
/// chunk from the mic pipeline, which triggers a manual commit with an
/// empty `audio_base_64`).
///
/// Base64 output only ever contains `[A-Za-z0-9+/=]`, none of which require
/// JSON string escaping, so `b64` is safe to write directly into the
/// manually built JSON text below.
fn build_audio_chunk_frame(b64: &str, commit: bool, out: &mut String) {
    out.clear();
    let _ = write!(
        out,
        r#"{{"message_type":"input_audio_chunk","audio_base_64":"{b64}","commit":{commit},"sample_rate":16000}}"#
    );
}

/// Open a WebSocket to the ElevenLabs Scribe v2 realtime STT endpoint.
/// Audio is base64-encoded as JSON frames; transcripts arrive as JSON messages.
/// Uses `commit_strategy=manual` so the caller controls when to finalize.
pub async fn start_realtime_session(
    api_key: &str,
    language: &str,
) -> Result<RealtimeSession> {
    let url = format!(
        "wss://api.elevenlabs.io/v1/speech-to-text/realtime\
         ?model_id=scribe_v2_realtime\
         &language_code={}\
         &audio_format=pcm_16000\
         &commit_strategy=manual",
        language
    );

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

    // Bounded, because `connect_async` imposes no timeout at any layer and a
    // stalled handshake would strand both callers: the recording loop, which
    // owns the hotkey receiver, and the startup warmup, which runs behind the
    // splash while the main window is still hidden.
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

    // Audio sender: encodes PCM → base64 JSON and streams to the WebSocket
    tokio::spawn(async move {
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
        // No sentinel convention on this channel — every event is ordinary
        // data, so a plain rate-limited `try_send` (no reserved headroom)
        // is sufficient.
        let mut dropped_transcripts: u64 = 0;
        let mut send_event = |ev: TranscriptEvent| {
            // Only warn on a genuinely full channel (consumer alive but
            // stalled). A `Closed` error means the orchestrator already
            // dropped `transcript_rx` (e.g. session teardown), which
            // happens on every session's trailing "WebSocket closed" Info
            // event — that's normal shutdown, not backpressure, so it's
            // dropped silently rather than logged as a bogus stall warning.
            match transcript_tx.try_send(ev) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn_channel_full(&mut dropped_transcripts, "ElevenLabs transcript");
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        };

        while let Some(Ok(msg)) = read.next().await {
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
                        Some("input_error") => {
                            let code = parsed["code"].as_str().unwrap_or("?");
                            let message = parsed["message"].as_str().unwrap_or("");
                            let err = format!("{} - {}", code, message);
                            tracing::error!("ElevenLabs input error: {}", err);
                            send_event(TranscriptEvent {
                                text: String::new(),
                                kind: TranscriptKind::Error(err),
                            });
                        }
                        other => {
                            tracing::debug!("Unknown WS message type: {:?}", other);
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

    Ok(RealtimeSession {
        audio_tx,
        transcript_rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manually built `input_audio_chunk` frame must be structurally
    /// identical to what the old `json!{...}` + `.to_string()` construction
    /// produced, for a representative non-empty chunk.
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

    /// The end-of-audio / manual-commit frame (empty chunk convention) must
    /// remain byte-semantically identical to the old `json!{...}` construction.
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

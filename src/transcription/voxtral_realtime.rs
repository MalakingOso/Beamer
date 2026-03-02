use anyhow::{Context, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tungstenite::Message;

use super::{RealtimeSession, TranscriptEvent, TranscriptKind};

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
        while let Some(chunk) = audio_rx.recv().await {
            if chunk.is_empty() {
                // Convention: empty Vec signals end-of-audio
                let msg = serde_json::json!({"type": "input_audio.end"});
                let _ = write.send(Message::Text(msg.to_string().into())).await;
                continue;
            }
            let msg = serde_json::json!({
                "type": "input_audio.append",
                "audio": engine.encode(&chunk),
            });
            if write
                .send(Message::Text(msg.to_string().into()))
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

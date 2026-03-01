use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tungstenite::Message;

use super::{RealtimeSession, TranscriptEvent, TranscriptKind, TranscriptionBackend};

pub struct ElevenLabsRealtime {
    api_key: String,
}

impl ElevenLabsRealtime {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl TranscriptionBackend for ElevenLabsRealtime {
    async fn transcribe_batch(
        &self,
        audio: Vec<u8>,
        language: &str,
        vocab: &[String],
    ) -> Result<String> {
        // Fall back to batch API for complete audio
        let batch = super::elevenlabs_batch::ElevenLabsBatch::new(self.api_key.clone());
        batch.transcribe_batch(audio, language, vocab).await
    }

    async fn start_realtime_session(&self, language: &str) -> Result<Option<RealtimeSession>> {
        let url = format!(
            "wss://api.elevenlabs.io/v1/speech-to-text/realtime\
             ?model_id=scribe_v2_realtime\
             &language_code={}\
             &audio_format=pcm_16000\
             &commit_strategy=manual",
            language
        );

        // Build request with auth header
        let request = tungstenite::http::Request::builder()
            .uri(&url)
            .header("xi-api-key", &self.api_key)
            .header("Host", "api.elevenlabs.io")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .context("Failed to build WebSocket request")?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .context("Failed to connect to ElevenLabs WebSocket")?;

        let (mut write, mut read) = ws_stream.split();

        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<TranscriptEvent>();

        // Send audio chunks as base64 JSON
        tokio::spawn(async move {
            let engine = base64::engine::general_purpose::STANDARD;
            while let Some(chunk) = audio_rx.recv().await {
                let msg = if chunk.is_empty() {
                    // Empty chunk = EOS: send commit
                    serde_json::json!({
                        "message_type": "input_audio_chunk",
                        "audio_base_64": "",
                        "commit": true,
                        "sample_rate": 16000
                    })
                } else {
                    serde_json::json!({
                        "message_type": "input_audio_chunk",
                        "audio_base_64": engine.encode(&chunk),
                        "commit": false,
                        "sample_rate": 16000
                    })
                };
                if write
                    .send(Message::Text(msg.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Receive transcriptions
        tokio::spawn(async move {
            while let Some(Ok(msg)) = read.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                        match parsed["message_type"].as_str() {
                            Some("session_started") => {
                                let sid = parsed["session_id"]
                                    .as_str()
                                    .unwrap_or("unknown")
                                    .to_string();
                                tracing::info!("ElevenLabs realtime session started: {}", sid);
                                let _ = transcript_tx.send(TranscriptEvent {
                                    text: String::new(),
                                    kind: TranscriptKind::SessionStarted(sid),
                                });
                            }
                            Some("partial_transcript") => {
                                let t = parsed["text"].as_str().unwrap_or("").to_string();
                                let _ = transcript_tx.send(TranscriptEvent {
                                    text: t,
                                    kind: TranscriptKind::Partial,
                                });
                            }
                            Some("committed_transcript") => {
                                let t = parsed["text"].as_str().unwrap_or("").to_string();
                                let _ = transcript_tx.send(TranscriptEvent {
                                    text: t,
                                    kind: TranscriptKind::Final,
                                });
                            }
                            Some("input_error") => {
                                let code = parsed["code"].as_str().unwrap_or("unknown");
                                let message = parsed["message"].as_str().unwrap_or("");
                                let err = format!("{} - {}", code, message);
                                tracing::error!("ElevenLabs input error: {}", err);
                                let _ = transcript_tx.send(TranscriptEvent {
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
            // WebSocket closed — notify orchestrator
            let _ = transcript_tx.send(TranscriptEvent {
                text: String::new(),
                kind: TranscriptKind::Info("WebSocket closed".to_string()),
            });
        });

        Ok(Some(RealtimeSession {
            audio_tx,
            transcript_rx,
        }))
    }
}

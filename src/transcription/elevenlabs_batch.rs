use anyhow::Result;
use async_trait::async_trait;
use reqwest::multipart;

use super::{RealtimeSession, TranscriptionBackend};

pub struct ElevenLabsBatch {
    api_key: String,
    client: reqwest::Client,
}

impl ElevenLabsBatch {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }

    fn build_form(audio: &[u8], language: &str, vocab: &[String]) -> Result<multipart::Form> {
        let file_part = multipart::Part::bytes(audio.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let mut form = multipart::Form::new()
            .part("file", file_part)
            .text("model_id", "scribe_v2")
            .text("language_code", language.to_string())
            .text("tag_audio_events", "false");

        if !vocab.is_empty() {
            let keyterms = serde_json::to_string(vocab)?;
            form = form.text("keyterms", keyterms);
        }

        Ok(form)
    }
}

#[async_trait]
impl TranscriptionBackend for ElevenLabsBatch {
    async fn transcribe_batch(
        &self,
        audio: Vec<u8>,
        language: &str,
        vocab: &[String],
    ) -> Result<String> {
        let mut retries = 0;
        let max_retries = 3;

        loop {
            let form = Self::build_form(&audio, language, vocab)?;

            let response = self
                .client
                .post("https://api.elevenlabs.io/v1/speech-to-text")
                .header("xi-api-key", &self.api_key)
                .multipart(form)
                .send()
                .await?;

            if response.status() == 429 && retries < max_retries {
                retries += 1;
                let delay = std::time::Duration::from_secs(1 << retries);
                tracing::warn!("Rate limited, retrying in {:?}", delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            let status = response.status();
            let body = response.text().await?;

            if !status.is_success() {
                anyhow::bail!("ElevenLabs API error {}: {}", status, body);
            }

            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            let text = parsed["text"].as_str().unwrap_or("").to_string();

            return Ok(text);
        }
    }

    async fn start_realtime_session(&self, _language: &str) -> Result<Option<RealtimeSession>> {
        Ok(None)
    }
}

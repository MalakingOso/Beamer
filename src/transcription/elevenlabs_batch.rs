use anyhow::{bail, Context, Result};
use reqwest::multipart;

use super::{http_client, wav::pcm_to_wav};
use bytes::Bytes;

/// Transcribe audio using the ElevenLabs Scribe v2 batch (REST) API.
///
/// `audio_pcm` must be raw 16-bit LE, 16 kHz, mono PCM. This function wraps it
/// in a WAV container before uploading. Vocabulary terms are sent as `keyterms`
/// (max 100, max 50 chars each).
pub async fn transcribe_batch(
    api_key: &str,
    audio_pcm: Vec<u8>,
    language: &str,
    vocab: &[String],
) -> Result<String> {
    let wav = pcm_to_wav(&audio_pcm);
    let wav_bytes = Bytes::from(wav);

    let client = http_client();
    let mut backoff = 1u64;

    for attempt in 0..4 {
        let mut form = multipart::Form::new()
            .text("model_id", "scribe_v2")
            .text("language_code", language.to_string())
            .text("tag_audio_events", "false")
            .part(
                "file",
                multipart::Part::stream(reqwest::Body::from(wav_bytes.clone()))
                    .file_name("audio.wav")
                    .mime_str("application/octet-stream")?,
            );

        // Send up to 100 keyterms (ElevenLabs limit)
        for term in vocab.iter().take(100) {
            if term.len() <= 50 {
                form = form.text("keyterms[]", term.clone());
            }
        }

        let resp = client
            .post("https://api.elevenlabs.io/v1/speech-to-text")
            .header("xi-api-key", api_key)
            // Without this the request is unbounded, and a stalled upload
            // strands the orchestrator's recording loop — which owns the
            // hotkey receiver, so dictation stops working entirely.
            .timeout(super::BATCH_REQUEST_TIMEOUT)
            .multipart(form)
            .send()
            .await
            .context("ElevenLabs batch request failed")?;

        if resp.status() == 429 {
            if attempt < 3 {
                tracing::warn!(
                    "ElevenLabs 429 rate-limited, retrying in {}s (attempt {})",
                    backoff,
                    attempt + 1
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(backoff)).await;
                backoff *= 2;
                continue;
            }
            bail!("ElevenLabs rate limit exceeded after {} retries", attempt);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("ElevenLabs batch API error {}: {}", status, body);
        }

        let json: serde_json::Value = resp.json().await.context("Failed to parse response")?;
        let text = json["text"].as_str().unwrap_or("").to_string();
        return Ok(text);
    }

    bail!("ElevenLabs batch request failed after retries")
}

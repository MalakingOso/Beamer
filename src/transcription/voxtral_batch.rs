use anyhow::{bail, Context, Result};
use reqwest::multipart;

use super::wav::pcm_to_wav;

/// Transcribe audio using the Mistral Voxtral batch (REST) API.
///
/// `audio_pcm` must be raw 16-bit LE, 16 kHz, mono PCM. Language is auto-detected.
pub async fn transcribe_batch(api_key: &str, audio_pcm: Vec<u8>) -> Result<String> {
    let wav = pcm_to_wav(&audio_pcm);

    let client = reqwest::Client::new();
    let mut backoff = 1u64;

    for attempt in 0..4 {
        let form = multipart::Form::new()
            .text("model", "voxtral-mini-latest")
            .part(
                "file",
                multipart::Part::bytes(wav.clone())
                    .file_name("audio.wav")
                    .mime_str("application/octet-stream")?,
            );

        let resp = client
            .post("https://api.mistral.ai/v1/audio/transcriptions")
            .header("x-api-key", api_key)
            .multipart(form)
            .send()
            .await
            .context("Voxtral batch request failed")?;

        if resp.status() == 429 {
            if attempt < 3 {
                tracing::warn!(
                    "Voxtral 429 rate-limited, retrying in {}s (attempt {})",
                    backoff,
                    attempt + 1
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(backoff)).await;
                backoff *= 2;
                continue;
            }
            bail!("Voxtral rate limit exceeded after {} retries", attempt);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Voxtral batch API error {}: {}", status, body);
        }

        let json: serde_json::Value = resp.json().await.context("Failed to parse response")?;
        let text = json["text"].as_str().unwrap_or("").to_string();
        return Ok(text);
    }

    bail!("Voxtral batch request failed after retries")
}

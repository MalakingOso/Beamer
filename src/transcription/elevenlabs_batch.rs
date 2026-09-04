use anyhow::{bail, Context, Result};
use reqwest::multipart;

use super::keyterms;
use super::{http_client, wav::pcm_to_wav};
use bytes::Bytes;

/// Transcribe raw 16-bit LE, 16 kHz, mono PCM via the ElevenLabs Scribe v2 batch API.
/// Wraps the PCM in a WAV container before uploading.
///
/// Vocabulary goes up as repeated `keyterms` multipart fields. ⚠️ The field is
/// `keyterms`, not `keyterms[]` — the bracketed spelling is silently ignored.
/// `no_verbatim` drops filler words and false starts.
pub async fn transcribe_batch(
    api_key: &str,
    audio_pcm: Vec<u8>,
    language: &str,
    vocab: &[String],
    no_verbatim: bool,
) -> Result<String> {
    let wav = pcm_to_wav(&audio_pcm);
    let wav_bytes = Bytes::from(wav);
    let terms = keyterms::sanitize(vocab, keyterms::BATCH_MAX_TERMS, keyterms::BATCH_MAX_CHARS);

    let client = http_client();
    let mut backoff = 1u64;

    for attempt in 0..4 {
        let mut form = multipart::Form::new()
            .text("model_id", "scribe_v2")
            .text("language_code", language.to_string())
            .text("tag_audio_events", "false")
            .text("no_verbatim", if no_verbatim { "true" } else { "false" })
            .part(
                "file",
                multipart::Part::stream(reqwest::Body::from(wav_bytes.clone()))
                    .file_name("audio.wav")
                    .mime_str("application/octet-stream")?,
            );

        for term in &terms {
            form = form.text("keyterms", term.clone());
        }

        let resp = client
            .post("https://api.elevenlabs.io/v1/speech-to-text")
            .header("xi-api-key", api_key)
            // A stalled upload would strand the recording loop (hotkey owner).
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

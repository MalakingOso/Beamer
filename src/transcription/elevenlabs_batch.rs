use anyhow::{bail, Context, Result};
use reqwest::multipart;

use super::keyterms;
use super::{http_client, wav::pcm_to_wav};
use bytes::Bytes;

/// Transcribe audio using the ElevenLabs Scribe v2 batch (REST) API.
///
/// `audio_pcm` must be raw 16-bit LE, 16 kHz, mono PCM. This function wraps it
/// in a WAV container before uploading.
///
/// Vocabulary terms go up as repeated `keyterms` multipart fields, sanitised by
/// `keyterms::sanitize` against the batch budget. ⚠️ The field name is
/// `keyterms`, **not** `keyterms[]`. Beamer sent the bracketed form until
/// 2026-08-23, and the server silently ignored it: a 60-character term (well
/// over the documented 50) came back `200 OK` under `keyterms[]` and
/// `400 "All keywords must be less than 50 characters"` under `keyterms`. The
/// bracketed spelling costs nothing and does nothing, so it looks like it
/// works. Verify with a deliberately invalid term, never with a plausible one.
///
/// `no_verbatim` asks the model to drop filler words, false starts and
/// disfluencies. Off unless the user turns it on.
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

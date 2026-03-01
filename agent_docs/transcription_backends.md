# Transcription Backends

## Trait Contract

```rust
#[async_trait]
pub trait TranscriptionBackend: Send + Sync {
    async fn transcribe_batch(&self, audio: Vec<u8>, language: &str, vocab: &[String]) -> Result<String>;
    async fn start_realtime_session(&self, language: &str) -> Result<Option<RealtimeSession>>;
}
```

## ElevenLabs Scribe v2 (Batch)

**Endpoint:** `POST https://api.elevenlabs.io/v1/speech-to-text`
**Auth:** `xi-api-key` header
**Content-Type:** `multipart/form-data`

Fields:
- `file` — WAV audio bytes
- `model_id` — `scribe_v2`
- `language_code` — ISO code (e.g., `en`)
- `tag_audio_events` — `false`
- `keyterms` — JSON array of vocabulary terms (max 100)

Response: `{ "text": "transcribed text" }`

Retry on HTTP 429 with exponential backoff (1s, 2s, 4s, max 3 retries).

## ElevenLabs Realtime (WebSocket)

**Endpoint:** `wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&language_code={lang}&audio_format=pcm_16000&commit_strategy=manual`
**Auth:** `xi-api-key` header on the WebSocket upgrade request

Connection flow:
1. Connect with `xi-api-key` header (no config message needed — all params in URL)
2. Send audio as JSON: `{ "message_type": "input_audio_chunk", "audio_base_64": "<b64>", "commit": false, "sample_rate": 16000 }`
3. Receive `{ "message_type": "session_started", "session_id": "..." }` on connect
4. Receive `{ "message_type": "partial_transcript", "text": "..." }` for partial results
5. Receive `{ "message_type": "committed_transcript", "text": "..." }` for final results
6. To end: send `{ "message_type": "input_audio_chunk", "audio_base_64": "", "commit": true, "sample_rate": 16000 }`
7. On error: `{ "message_type": "input_error", "code": "...", "message": "..." }`

Partial transcripts update overlay. Committed transcripts trigger injection.

Commit strategy is `manual` — Beamer has its own VAD, so we commit explicitly on end-of-speech.

## Backend Factory

```rust
pub fn create_backend(backend_name: &str, api_key: String) -> Box<dyn TranscriptionBackend> {
    match backend_name {
        "elevenlabs_realtime" => Box::new(ElevenLabsRealtime::new(api_key)),
        _ => Box::new(ElevenLabsBatch::new(api_key)), // default
    }
}
```

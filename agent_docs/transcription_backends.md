# Transcription Backends

## No Trait

There is no `TranscriptionBackend` trait and no backend factory. Each
backend is a plain async function in its own module under
`src/transcription/`:

```
elevenlabs_batch.rs    transcribe_batch(api_key, audio_pcm, language, vocab) -> Result<String>
elevenlabs_realtime.rs start_realtime_session(api_key, language) -> Result<RealtimeSession>
voxtral_batch.rs       transcribe_batch(api_key, audio_pcm, vocab) -> Result<String>
voxtral_realtime.rs    start_realtime_session(api_key) -> Result<RealtimeSession>
```

`mod.rs` re-exports these under distinguishing names
(`transcribe_batch`/`transcribe_voxtral_batch`,
`start_elevenlabs_session`/`start_voxtral_session`). Backend selection is a
plain string match in `orchestrator.rs` on `cfg.transcription.backend`
(`"elevenlabs"` default — ElevenLabs realtime, per `default_backend()` in
`src/config/mod.rs` — plus `"elevenlabs_batch"`, `"voxtral_batch"`, and
`"voxtral"` for Voxtral realtime; anything else falls through to ElevenLabs
realtime).

`RealtimeSession` (defined in `mod.rs`) is the only shared abstraction:

```rust
pub struct RealtimeSession {
    pub audio_tx: mpsc::Sender<Vec<u8>>,
    pub transcript_rx: mpsc::Receiver<TranscriptEvent>,
}
```

Both realtime backends produce one of these and normalize their own
wire-format messages into the shared `TranscriptEvent { text, kind }` /
`TranscriptKind` (`Partial`, `Final`, `SessionStarted(String)`,
`Error(String)`, `Info(String)`) enum.

## Shared Infrastructure (`mod.rs`)

- `http_client()` — a single lazily-built `reqwest::Client` behind a
  `OnceLock`, shared by all four backends so connection pools/TLS contexts
  aren't rebuilt per request.
- Bounded channel capacities, sized off the same worst-case cadence as the
  mic-capture path (see `audio_pipeline.md`):
  - `AUDIO_CHANNEL_CAPACITY = 12_000` — `RealtimeSession::audio_tx`, fed 1:1
    from `AudioPipeline`'s PCM chunks.
  - `AUDIO_SENTINEL_RESERVE = 4` — slots withheld on `audio_tx` so the
    end-of-audio sentinel always has room via `try_send_reserving`, even if
    a stalled WebSocket write has saturated the data path.
  - `TRANSCRIPT_CHANNEL_CAPACITY = 1_200` — `transcript_rx`, sized for a
    conservative 20Hz upper bound on partial-transcript pushes (both
    providers typically emit every 100–300ms). No sentinel travels on this
    channel, so sends are a plain rate-limited `try_send` +
    `warn_channel_full` (no reserved headroom needed).

## End-of-Audio Convention

Both realtime backends' audio-sender tasks treat an **empty `Vec<u8>`** sent
on `audio_tx` as "finalize now": `orchestrator.rs` sends one after the
~400ms capture tail on record-stop. Each backend translates that into its
own wire message (ElevenLabs: `commit: true` with empty `audio_base_64`;
Voxtral: a dedicated `input_audio.end` message).

Frame-building for outgoing audio messages is hand-rolled string formatting
into a reused `String` buffer (`build_audio_chunk_frame` /
`build_audio_frame`), not `serde_json::json!{...}.to_string()` per chunk —
this is a recent perf change. It's safe because base64 output only ever
contains `[A-Za-z0-9+/=]`, none of which need JSON escaping.

## ElevenLabs Scribe v2 (Batch) — `elevenlabs_batch.rs`

**Endpoint:** `POST https://api.elevenlabs.io/v1/speech-to-text`
**Auth:** `xi-api-key` header
**Content-Type:** `multipart/form-data`

Fields:
- `file` — WAV bytes (`transcription::wav::pcm_to_wav` wraps the raw PCM;
  no `hound` dependency)
- `model_id` — `scribe_v2`
- `language_code` — ISO code (e.g. `en`)
- `tag_audio_events` — `false`
- `keyterms[]` — **repeated form field**, one per vocab term, up to 100
  terms, each ≤50 chars. Not a JSON array — each term is its own
  `keyterms[]` multipart field.

Retry on HTTP 429 with exponential backoff (1s, 2s, 4s; up to 3 retries).
The WAV body is wrapped in `bytes::Bytes` once up front and cheaply cloned
(refcount bump, not a copy) for each retry attempt instead of re-reading it.

Response: `{ "text": "..." }`.

## ElevenLabs Realtime (WebSocket) — `elevenlabs_realtime.rs`

**Endpoint:** `wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&language_code={lang}&audio_format=pcm_16000&commit_strategy=manual`
**Auth:** `xi-api-key` header on the WebSocket upgrade request (all config
is in the URL — no separate config message needed after connect).

Message flow:
1. Connect; server sends `{ "message_type": "session_started", "session_id": "..." }`.
2. Client sends audio: `{ "message_type": "input_audio_chunk", "audio_base_64": "<b64>", "commit": false, "sample_rate": 16000 }`.
3. Server sends `{ "message_type": "partial_transcript", "text": "..." }` for interim results (→ `TranscriptKind::Partial`).
4. Server sends `{ "message_type": "committed_transcript", "text": "..." }` for finals (→ `TranscriptKind::Final`).
5. End-of-audio: client sends `{ "message_type": "input_audio_chunk", "audio_base_64": "", "commit": true, "sample_rate": 16000 }`.
6. Errors arrive as `{ "message_type": "input_error", "code": "...", "message": "..." }` (→ `TranscriptKind::Error`).

`commit_strategy=manual` is used because Beamer decides when to finalize
(hotkey release), not the server.

## Mistral Voxtral (Batch) — `voxtral_batch.rs`

**Endpoint:** `POST https://api.mistral.ai/v1/audio/transcriptions`
**Auth:** `x-api-key` header (note: **not** `Authorization: Bearer` — that's
only used by the vocab-correction call below)
**Content-Type:** `multipart/form-data`

Fields: `model` = `voxtral-mini-latest`, `file` = WAV bytes. Language is
auto-detected — no `language_code` field. Same 429 retry/backoff and
`Bytes`-reuse behavior as the ElevenLabs batch path.

**Vocabulary correction (Voxtral-only):** if `vocab` is non-empty, the raw
transcript is post-processed with a second call to
`POST https://api.mistral.ai/v1/chat/completions` (`model:
mistral-small-latest`, `Authorization: Bearer {api_key}`, `temperature: 0`).
The system prompt is deliberately defensive — it frames the model as a
"transcription processor, not an assistant" so dictated content that looks
like an instruction doesn't get obeyed. The corrected text is accepted only
if its Levenshtein-based similarity to the original is ≥ `MIN_SIMILARITY`
(0.5); below that it's discarded as a likely hallucinated reply and the raw
transcript is used instead. Any request/parse failure also falls back to
the raw transcript.

## Mistral Voxtral (Realtime) — `voxtral_realtime.rs`

**Endpoint:** `wss://api.mistral.ai/v1/audio/transcriptions/realtime?model=voxtral-mini-transcribe-realtime-2602`
**Auth:** `Authorization: Bearer {api_key}` header on the upgrade request.

Unlike ElevenLabs, Voxtral requires an explicit config message right after
connecting, before any audio:
```json
{"type": "session.update", "session": {"audio_format": {"encoding": "pcm_s16le", "sample_rate": 16000}}}
```

Message flow:
- `session.created` → `TranscriptKind::SessionStarted(request_id)`
- `session.updated` → logged only (no event)
- `transcription.text.delta` → `TranscriptKind::Partial`
- `transcription.done` → `TranscriptKind::Final`
- `transcription.language` → `TranscriptKind::Info("Language: ...")`
- `transcription.segment` → logged only (no event)
- `error` → `TranscriptKind::Error`
- Client audio: `{"type": "input_audio.append", "audio": "<b64>"}`
- End-of-audio: `{"type": "input_audio.end"}` (empty-chunk convention)

## `wav.rs`

`pcm_to_wav(pcm: &[u8]) -> Vec<u8>` writes a minimal 44-byte RIFF/WAVE header
(16-bit PCM, 16 kHz, mono) by hand — no `hound` or other WAV crate. Used
only at batch-upload time, never during capture or storage.

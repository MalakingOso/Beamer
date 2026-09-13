# Transcription Backends

## No Trait

There is no `TranscriptionBackend` trait and no backend factory. Each
backend is a plain async function in its own module under
`src/transcription/`:

```
elevenlabs_batch.rs    transcribe_batch(api_key, audio_pcm, language, vocab, no_verbatim) -> Result<String>
elevenlabs_realtime.rs start_realtime_session(api_key, language, vocab, no_verbatim) -> Result<RealtimeSession>
voxtral_batch.rs       transcribe_batch(api_key, audio_pcm, vocab) -> Result<String>
voxtral_realtime.rs    start_realtime_session(api_key) -> Result<RealtimeSession>
```

`mod.rs` re-exports these under distinguishing names
(`transcribe_batch`/`transcribe_voxtral_batch`,
`start_elevenlabs_session`/`start_voxtral_session`). Backend selection is a
plain string match in `orchestrator.rs` on `cfg.transcription.backend`
(`"elevenlabs"` default — ElevenLabs realtime, per `default_backend()` in
`src/config/mod.rs` — plus `"elevenlabs_batch"`, `"voxtral_batch"`, and
`"voxtral"` for Voxtral realtime; unknown backends are rejected with an error).

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
- `no_verbatim` — `cfg.transcription.no_verbatim`, default `false`
- `keyterms` — **repeated form field**, one per vocab term. Not a JSON
  array — each term is its own `keyterms` multipart field.

⚠️ **The field is `keyterms`, not `keyterms[]`.** Beamer sent the bracketed
spelling until 2026-08-23 and the server discarded it in silence — verified by
sending a 60-character term (over the documented 50-char limit) both ways:

| Field name | Response |
|---|---|
| `keyterms[]` | `200 OK`, transcript returned, invalid term never mentioned |
| `keyterms` | `400 {"message":"All keywords must be less than 50 characters."}` |

Validation is the only signal that a field was read at all. If you ever need to
confirm ElevenLabs is honouring a parameter, send a *deliberately invalid*
value — a plausible one tells you nothing, because being ignored and being
accepted look identical.

Retry on HTTP 429 with exponential backoff (1s, 2s, 4s; up to 3 retries).
The WAV body is wrapped in `bytes::Bytes` once up front and cheaply cloned
(refcount bump, not a copy) for each retry attempt instead of re-reading it.

Response: `{ "text": "..." }`.

## Keyterms — `keyterms.rs`

Both ElevenLabs backends share one pure function,
`keyterms::sanitize(terms, max_terms, max_chars)`, because every rule the API
imposes rejects the **whole request** rather than the offending term. Dropping
a term silently costs one dictation's accuracy; a 400 costs the dictation.

Rules enforced (all the API's, none invented here): non-empty after trimming,
≤ `max_chars` **characters** (not bytes), ≤ 5 words, none of `< > { } [ ] \`,
de-duplicated, then capped at `max_terms`. The cap counts *survivors*, so a run
of rejects at the front of the vocabulary doesn't eat the budget.

| | Batch (`scribe_v2`) | Realtime (`scribe_v2_realtime`) |
|---|---|---|
| Terms | `BATCH_MAX_TERMS` = **100** | `REALTIME_MAX_TERMS` = **50** |
| Chars | `BATCH_MAX_CHARS` = **49** | `REALTIME_MAX_CHARS` = **20** |

Two constants look wrong and aren't:

- **Batch is 49, not 50.** The API words the limit as "must be *less than* 50
  characters", so 50 is a rejection.
- **Batch is capped at 100, not the documented 1000.** More than 100 keyterms
  triggers a **20-second minimum billable duration** per request. Beamer's
  utterances are seconds long, so raising this multiplies the bill for a
  benefit no dictation-length clip can collect. Price it before changing it.

Keyterm prompting also carries a surcharge in its own right, which is why
`warmup.rs` passes an **empty** vocab: that session's transcript is discarded.

## ElevenLabs Realtime (WebSocket) — `elevenlabs_realtime.rs`

**Endpoint:** `wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&language_code={lang}&audio_format=pcm_16000&commit_strategy=manual[&no_verbatim=true][&keyterms=…]*`
**Auth:** `xi-api-key` header on the WebSocket upgrade request (all config
is in the URL — no separate config message needed after connect).

`build_realtime_url()` is a pure function with its exact output pinned by unit
tests. Keyterms are **repeated `&keyterms=` query parameters**, percent-encoded
(`encode_query_value`) because a keyterm may legitimately contain a space
(`"Beamer Purple"` → `keyterms=Beamer%20Purple`). `no_verbatim=true` is appended
only when enabled, so an untouched install produces byte-identical URLs to the
pre-2026-08-23 build.

**Verifying a realtime parameter costs nothing.** `session_started` echoes back
the config the server actually parsed, so connecting and reading one message —
sending no audio at all — confirms the encoding without a billable transcript:

```json
{"message_type":"session_started","session_id":"…","config":{
  "keyterms":["Beamer","Beamer Purple"],"no_verbatim":true,
  "filter_background_audio":true,"secondary_languages":[],"entity_detection":null, …}}
```

That echo also lists parameters not in the published AsyncAPI spec
(`secondary_languages`, `timestamps_granularity`, `max_tokens_to_recompute`,
`vad_commit_strategy`, `disable_logging`) — it's the most current description
of the endpoint available.

Message flow:
1. Connect; server sends `{ "message_type": "session_started", "session_id": "..." }`.
2. Client sends audio: `{ "message_type": "input_audio_chunk", "audio_base_64": "<b64>", "commit": false, "sample_rate": 16000 }`.
3. Server sends `{ "message_type": "partial_transcript", "text": "..." }` for interim results (→ `TranscriptKind::Partial`).
4. Server sends `{ "message_type": "committed_transcript", "text": "..." }` for finals (→ `TranscriptKind::Final`).
5. End-of-audio: client sends `{ "message_type": "input_audio_chunk", "audio_base_64": "", "commit": true, "sample_rate": 16000 }`.
6. Errors arrive as `{ "message_type": "<kind>", "error": "..." }` (→ `TranscriptKind::Error`).

⚠️ **There is no single error message type.** The API defines a separate
`message_type` per failure — `error`, `auth_error`, `quota_exceeded`,
`rate_limited`, `commit_throttled`, `unaccepted_terms`, `queue_overflow`,
`resource_exhausted`, `session_time_limit_exceeded`, `input_error`,
`invalid_request`, `chunk_size_exceeded`, `insufficient_audio_activity`,
`transcriber_error` — each carrying one `error` string. Beamer used to match
only `input_error` and read `code`/`message`, fields the API does not send, so
an input error rendered as `"? - "` and everything else (a blown quota, an
expired key) was logged at debug and swallowed. The user saw dictation produce
nothing, with no reason given.

`describe_error()` therefore recognises the *shape* rather than enumerating the
list: anything that isn't one of `NON_ERROR_MESSAGE_TYPES` and carries an error
string is surfaced. New error types added upstream are covered automatically;
add to `NON_ERROR_MESSAGE_TYPES` only when a *non*-error message type appears.

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

## ElevenLabs API review — reviewed and deliberately skipped

The full Scribe v2 surface was surveyed on **2026-08-23** (batch reference,
realtime AsyncAPI spec, capability page, keyterm guide). Adopted: `keyterms` on
both paths, `no_verbatim`. Everything below was read and rejected *for this
app* — don't re-derive the list, and don't add one without a reason that
survives the note next to it.

| Parameter | Why not |
|---|---|
| `entity_detection` / `entity_redaction` | Detects and masks credit cards, SSNs, names. Beamer injects into whatever field has focus; redacting the user's own dictation is the opposite of the job. |
| `diarize`, `num_speakers`, `use_speaker_library`, `detect_speaker_roles` | Single-speaker push-to-talk. Meeting capture is an explicit non-goal. |
| `use_multi_channel`, `multichannel_output_style` | Mic capture is mono by construction (`audio_pipeline.md`). |
| `webhook`, `webhook_id`, `webhook_metadata` | Asynchronous delivery to a public endpoint. Beamer wants the text now, in-process. |
| `temperature`, `seed` | Determinism knobs for evaluation. Dictation wants the model's best guess; the default temperature already is one. |
| `secondary_languages` | Code-switching between two named languages. No demand yet — revisit if bilingual dictation comes up. |
| `include_timestamps`, `timestamps_granularity` | Word timings are for subtitles and editors. Beamer discards everything but `text`. |
| `filter_background_audio` (realtime) | **Verified accepted** by the live endpoint, and plausibly useful on a desktop mic. Left off because it is untested against real recordings — enabling it is a one-line change if speech is being lost to room noise. |
| `enable_logging=false` (zero retention) | Enterprise-only on the ElevenLabs side. |
| Single-use tokens (`tokens.singleUse.create`) | Exists so browsers never see the API key. Beamer is a desktop client holding the key in the OS keyring already. |
| `source_url`, `cloud_storage_url`, `file_format`, `additional_formats` | File/URL-oriented batch transcription, not live capture. |

Not adopted for a different reason: **Voxtral realtime keyterms**. Mistral's
realtime endpoint exposes no vocabulary mechanism, so `voxtral_realtime.rs`
still ignores vocabulary. `voxtral_batch.rs` compensates with the LLM
correction pass; realtime has no equivalent and this is a known gap, not an
oversight.

## `wav.rs`

`pcm_to_wav(pcm: &[u8]) -> Vec<u8>` writes a minimal 44-byte RIFF/WAVE header
(16-bit PCM, 16 kHz, mono) by hand — no `hound` or other WAV crate. Used
only at batch-upload time, never during capture or storage.

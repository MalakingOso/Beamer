# Transcription Backends

## No Trait

There is no `TranscriptionBackend` trait and no backend factory. Each
backend is a plain async function in its own module under
`src/transcription/`:

```
elevenlabs_batch.rs    transcribe_batch(api_key, audio_pcm, language, vocab, no_verbatim) -> Result<String>
                       transcribe_medical_batch(same args) -> Result<String>
voxtral_batch.rs       transcribe_batch(api_key, audio_pcm, vocab) -> Result<String>
```

`mod.rs` re-exports the ElevenLabs pair as `transcribe_batch` /
`transcribe_medical_batch` and the Voxtral one as `transcribe_voxtral_batch`.
Backend selection is a plain string match in `orchestrator/mod.rs` on
`cfg.transcription.backend` (`"elevenlabs_batch"` default, per `default_backend()`
in `src/config/mod.rs` — plus `"elevenlabs_medical_batch"` and `"voxtral_batch"`;
unknown backends are rejected with an error, never silently remapped).
A stored `"elevenlabs"` / `"voxtral"` from before the realtime removal migrates
to its `_batch` counterpart on load (see `migrate_transcription_backend`).

There is no shared backend trait or session abstraction: every recording
buffers mic PCM and POSTs it to one batch endpoint. The only shared pieces
are the HTTP client, the batch timeout/retry helpers, and the WAV/keyterm
preparation in `mod.rs` / `wav.rs` / `keyterms.rs`.

## Shared Infrastructure (`mod.rs`)

- `http_client()` — a single lazily-built `reqwest::Client` behind a
  `OnceLock`, shared by all three backends so connection pools/TLS contexts
  aren't rebuilt per request.
- Timeouts, three layers: `CONNECT_TIMEOUT` (10s, TCP+TLS open),
  `BATCH_REQUEST_TIMEOUT` (120s, one request end to end),
  `BATCH_OVERALL_TIMEOUT` (480s, the whole transcription including retries
  and the Voxtral vocab-correction call). The overall ceiling exists because
  the orchestrator holds the hotkey receiver while a transcript is in flight,
  so an unbounded stall strands every later hotkey event.
- `batch_should_retry(status)` — 429 and 5xx retry; anything else (auth,
  model, malformed request) fails identically on retry and doesn't.

## ElevenLabs Scribe v2 (Batch) — `elevenlabs_batch.rs`

**Endpoint:** `POST https://api.elevenlabs.io/v1/speech-to-text`
**Auth:** `xi-api-key` header
**Content-Type:** `multipart/form-data`

Fields:
- `file` — WAV bytes (`transcription::wav::pcm_to_wav` wraps the raw PCM;
  no `hound` dependency)
- `model_id` — `scribe_v2` (`elevenlabs_batch`) or `scribe_v2_medical`
  (`elevenlabs_medical_batch`). The medical model is a separate model, not a
  mode: same endpoint, same fields, same response shape — only the id changes.
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

Timeouts, connect errors, 429 and 5xx are retried up to 3 times (1s, 2s, 4s
backoff) within `BATCH_OVERALL_TIMEOUT` — see "Shared Infrastructure" above.
The WAV body is wrapped in `bytes::Bytes` once up front and cheaply cloned
(refcount bump, not a copy) for each retry attempt instead of re-reading it.

Response: `{ "text": "..." }`.

## Keyterms — `keyterms.rs`

Both ElevenLabs batch backends share one pure function,
`keyterms::sanitize(terms, max_terms, max_chars)`, because every rule the API
imposes rejects the **whole request** rather than the offending term. Dropping
a term silently costs one dictation's accuracy; a 400 costs the dictation.

Rules enforced (all the API's, none invented here): non-empty after trimming,
≤ `max_chars` **characters** (not bytes), ≤ 5 words, none of `< > { } [ ] \`,
de-duplicated, then capped at `max_terms`. The cap counts *survivors*, so a run
of rejects at the front of the vocabulary doesn't eat the budget.

| | Batch (`scribe_v2` / `scribe_v2_medical`) |
|---|---|
| Terms | `BATCH_MAX_TERMS` = **100** |
| Chars | `BATCH_MAX_CHARS` = **49** |

Two constants look wrong and aren't:

- **Batch is 49, not 50.** The API words the limit as "must be *less than* 50
  characters", so 50 is a rejection.
- **Batch is capped at 100, not the documented 1000.** More than 100 keyterms
  triggers a **20-second minimum billable duration** per request. Beamer's
  utterances are seconds long, so raising this multiplies the bill for a
  benefit no dictation-length clip can collect. Price it before changing it.

## Mistral Voxtral (Batch) — `voxtral_batch.rs`

**Endpoint:** `POST https://api.mistral.ai/v1/audio/transcriptions`
**Auth:** `x-api-key` header (note: **not** `Authorization: Bearer` — that's
only used by the vocab-correction call below)
**Content-Type:** `multipart/form-data`

Fields: `model` = `voxtral-mini-latest`, `file` = WAV bytes. Language is
auto-detected — no `language_code` field. Same retry policy, timeouts and
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
| `enable_logging=false` (zero retention) | Enterprise-only on the ElevenLabs side. |
| Single-use tokens (`tokens.singleUse.create`) | Exists so browsers never see the API key. Beamer is a desktop client holding the key in the OS keyring already. |
| `source_url`, `cloud_storage_url`, `file_format`, `additional_formats` | File/URL-oriented batch transcription, not live capture. |

Vocabulary on Voxtral works differently: the batch endpoint exposes no
keyterm mechanism, so `voxtral_batch.rs` compensates with the LLM correction
pass described above.

## `wav.rs`

`pcm_to_wav(pcm: &[u8]) -> Vec<u8>` writes a minimal 44-byte RIFF/WAVE header
(16-bit PCM, 16 kHz, mono) by hand — no `hound` or other WAV crate. Used
only at batch-upload time, never during capture or storage.

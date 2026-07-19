# Audio Pipeline

## Overview

There is no VAD, no ring buffer, and no `AudioEvent` enum — none of that was
ever built. The actual flow is two stages connected by a bounded channel:

```
cpal input stream (src/audio/capture.rs)
  → downmix + resample, done inline in the audio callback
  → mpsc::Sender<Vec<f32>>  (SAMPLE_CHANNEL_CAPACITY)
      ↓
chunker thread (src/audio/mod.rs, AudioPipeline::start)
  → f32 → i16 LE PCM conversion, live-level RMS publish
  → mpsc::Sender<Vec<u8>>  (CHUNK_CHANNEL_CAPACITY)
      ↓
orchestrator.rs
  → realtime: forwarded 1:1 into RealtimeSession::audio_tx (WebSocket)
  → batch: concatenated into one Vec<u8>, wrapped in a WAV header at send time
```

`AudioPipeline::start()` returns `(cpal::Stream, mpsc::Receiver<Vec<u8>>)`.
The caller must keep the `Stream` alive — dropping it stops capture.

## cpal Setup (`src/audio/capture.rs`)

`AudioCapture::new()` grabs `cpal::default_host().default_input_device()`
and targets 16 kHz mono. If the device's native config already matches, it
captures directly at 16 kHz/1ch. Otherwise it captures at the device's
**native** rate/channel count and does downmix + resample **inside the cpal
callback itself**, using closure-owned state (`ResampleState { accumulator,
last_sample }`, plus reused `mono_buf`/`out_buf` scratch `Vec`s — no
per-callback allocation).

- Downmix: average all channels per frame (`data.chunks(native_channels)`).
- Resample: hand-rolled **linear interpolation** (`resample_linear`) — no
  `rubato`, no other resampling crate. State persists across callbacks so
  there's no discontinuity at buffer boundaries. Good enough for speech;
  not broadcast-quality.

The callback never blocks: it hands samples off via `try_send_reserving`
(see below) and returns immediately, which matters because it runs on a
real-time audio thread (WASAPI/CoreAudio/PulseAudio/PipeWire callback).

On a capture error, cpal's error callback sends an **empty `Vec<f32>`** as a
sentinel. It is delivered to the chunker thread, but that consumer currently
just skips empty batches and continues (see below) — it doesn't otherwise
act on the error today. This preserves the pre-branch drop-and-continue
behavior rather than being unused plumbing; a future consumer could still
read the empty batch as an explicit device-error signal instead of treating
it as a plain empty chunk.

## Chunker Thread (`src/audio/mod.rs`)

`AudioPipeline::start()` spawns a dedicated `std::thread` (not a tokio task)
that `blocking_recv()`s raw f32 sample batches off the capture channel,
because cpal callbacks and this thread are outside the async runtime. Per
batch it:

1. Skips empty batches without publishing a level update, then breaks out
   when the channel closes (stream torn down) — an empty batch there is a
   `Some(vec![])` sentinel from `capture.rs`'s error path, not end-of-stream.
2. Computes RMS, maps it to a display level (`normalize_rms`, 8x gain —
   typical speech RMS is ~0.12 so this puts speech near full scale), and
   publishes it on a `watch::channel` (`subscribe_levels()`). This is what
   drives the GNOME Shell pill's waveform via `linux_integration.rs`'s 66ms
   poll loop.
3. Converts f32 `[-1.0, 1.0]` samples to **16-bit LE PCM** (`f32_to_i16_bytes`,
   clamping out-of-range values) and forwards the bytes to the output
   channel.

There is no WAV encoding here and no `hound` dependency for capture — chunks
are raw PCM. WAV wrapping only happens once, in
`transcription::wav::pcm_to_wav`, right before a batch upload (see
`transcription_backends.md`).

## Bounded Channels + Sentinel Headroom

Both the raw-sample channel (`capture.rs`) and the converted-PCM channel
(`mod.rs`) are **bounded**, sized for ≥60s of audio at the fastest realistic
cpal callback cadence (5ms/callback → 200/sec):

```
SAMPLE_CHANNEL_CAPACITY = CHUNK_CHANNEL_CAPACITY = 60 * 200 = 12_000
```

Unbounded channels were replaced with these bounded ones so a stalled
consumer drops audio instead of growing memory without limit. Two helpers in
`src/audio/mod.rs` are shared across the audio *and* transcription channels:

- `try_send_reserving(tx, reserve, payload)` — a `try_send` gated on cheap
  atomic `tx.capacity()`/`tx.is_closed()` reads (safe to call from the
  real-time callback). It refuses to touch the last `reserve` slots, so a
  **sentinel** sent later with a plain `tx.try_send(..)` always has room
  even if the data path has saturated everything else. `capture.rs`
  reserves `SENTINEL_RESERVE = 4` slots for its device-error sentinel (an
  empty `Vec<f32>`). Returns a `SendOutcome` (`Sent`/`Full`/`Closed`) rather
  than a plain bool, so callers can warn on a genuinely full channel while
  silently dropping sends on a closed one (normal teardown, not
  backpressure).
- `warn_channel_full(&mut count, what)` — rate-limited drop logging: warns
  on the first drop, then every 200th, so a sustained stall produces one log
  line plus periodic reminders instead of flooding the log.

The converted-PCM channel (`mod.rs`) does **not** need reserved headroom —
the empty-`Vec<f32>` sentinel from the capture side is filtered out
(`if samples.is_empty() { continue; }`) before it would ever reach the PCM
channel, so no sentinel travels on it.

## Live Mic Level

`src/audio/mod.rs` exposes `subscribe_levels() -> watch::Receiver<f32>`
(0.0 = silence, 1.0 = loud speech). Published once per converted chunk from
the chunker thread. Consumed by `src/ui/linux_integration.rs`, which polls
it on a fixed 66ms tick (`borrow_and_update`) to drive the GNOME Shell pill's
waveform — polling rather than `changed().await` because the audio callback
publishes at 100+ Hz and there's no reason to wake that often just to
throttle back down to ~15 Hz.

## Consumers

`orchestrator.rs` owns the `cpal::Stream` and drains the PCM-chunk
`Receiver`:

- **Realtime mode**: each non-empty chunk is forwarded via
  `try_send_reserving` into the active `RealtimeSession::audio_tx`. On
  record-stop, an empty `Vec<u8>` is sent as the end-of-audio convention (see
  `transcription_backends.md`) using the reserved headroom on that channel.
- **Batch mode**: chunks are simply concatenated into one `Vec<u8>` PCM
  buffer until record-stop (plus a ~400ms tail to avoid clipping the last
  word), then handed to `transcription::wav::pcm_to_wav` + a batch API call.

There is no `SpeechStart`/`SpeechEnd`/overlay-glow wiring anywhere in this
path — recording-state UI (pill, shell indicator) is driven from
`orchestrator::RecordingState`, set directly around hotkey events, not from
audio-derived speech detection.

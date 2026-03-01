# Audio Pipeline

## Overview

Audio flows: Microphone → cpal → Ring Buffer → VAD → WAV Encoder → Transcription Backend

## cpal Setup

1. Get default input device: `cpal::default_host().default_input_device()`
2. Get supported input config (prefer 16kHz mono f32)
3. Build input stream with callback that pushes samples to a channel
4. Handle device disconnection gracefully

## Sample Rate Conversion

Most mics output 44.1kHz or 48kHz. VAD and transcription backends need 16kHz.

If device sample rate != 16000:
- Use `rubato` crate for high-quality resampling
- Or simple linear interpolation for lower quality but zero dependencies
- Resample in chunks matching VAD frame size

## VAD (Voice Activity Detection)

Using `webrtc-vad` crate:
- Frame sizes: 10ms, 20ms, or 30ms at 16kHz
- 10ms = 160 samples, 20ms = 320 samples, 30ms = 480 samples
- Aggressiveness: mode 2 (balanced) or mode 3 (aggressive, less false positives)

State machine:
```
Idle → [speech detected for 3+ frames] → Speaking
Speaking → [silence detected for 30+ frames (~600ms)] → SpeechEnd
SpeechEnd → emit AudioReady(wav_bytes) → Idle
```

Pre-buffer: keep last 300ms of audio before speech detection triggers, prepend to speech buffer.

## WAV Encoding

Use `hound` crate to encode to WAV:
```rust
let spec = WavSpec {
    channels: 1,
    sample_rate: 16000,
    bits_per_sample: 16,
    sample_format: SampleFormat::Int,
};
let mut cursor = Cursor::new(Vec::new());
let mut writer = WavWriter::new(&mut cursor, spec)?;
for sample in samples {
    writer.write_sample((sample * 32767.0) as i16)?;
}
writer.finalize()?;
cursor.into_inner() // WAV bytes
```

## Buffer Sizing

- Ring buffer: 30 seconds of audio at 16kHz mono = 960,000 samples (~1.8MB f32)
- VAD frame buffer: 320 samples (20ms)
- Pre-buffer: 4800 samples (300ms)

## AudioEvent Enum

```rust
pub enum AudioEvent {
    SpeechStart,
    SpeechEnd,
    AudioReady(Vec<u8>),      // WAV bytes for batch
    AudioChunk(Vec<u8>),      // Raw PCM chunk for realtime streaming
    DeviceError(String),
}
```

## Channel Architecture

```
AudioPipeline::start() -> mpsc::Receiver<AudioEvent>
```

The receiver is consumed by the orchestrator in main.rs which routes events:
- SpeechStart → show overlay, start glow
- AudioChunk → forward to realtime WebSocket
- AudioReady → send to batch transcription
- SpeechEnd → hide glow (overlay hides after injection)

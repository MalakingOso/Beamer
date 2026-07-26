use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SampleFormat, SampleRate, SizedSample, Stream, StreamConfig};
use tokio::sync::mpsc;

use super::{try_send_reserving, warn_channel_full, SendOutcome};

/// Bounded capacity for the raw-sample channel from the cpal callback.
///
/// cpal's `BufferSize::Default` hands buffer-size (and therefore callback
/// cadence) selection to the host audio API. Measured/typical callback
/// periods across WASAPI (Windows), CoreAudio (macOS) and PulseAudio/
/// PipeWire (Linux) commonly fall in the 5-20ms range depending on the
/// device and host. We size the channel for the fastest realistic cadence
/// (5ms per callback -> 200 callbacks/sec) so it holds >=60s of audio even
/// on the device with the shortest observed callback period; on a device
/// with a longer period this buffers correspondingly *more* than 60s of
/// audio, which is still bounded and therefore fine.
///
///   60s * (1000ms/s / 5ms per callback) = 60 * 200 = 12_000
pub(crate) const SAMPLE_CHANNEL_CAPACITY: usize = 12_000;

/// Slots permanently withheld from ordinary sample data so the rare
/// device-error sentinel (an empty `Vec` sent from the cpal error callback)
/// always has room to `try_send`, even when a stalled consumer has let the
/// data path saturate the rest of the channel. Device errors are
/// exceedingly rare (not per-callback), so a small reserve is ample. Note
/// that the sentinel is currently just skipped by its consumer once
/// delivered (see the error-callback comment below) — the reserve still
/// matters because it's what guarantees delivery isn't lost to backpressure.
const SENTINEL_RESERVE: usize = 4;

/// Wraps cpal device setup and provides a mono 16 kHz f32 sample stream.
/// Resamples from the device's native rate when it differs from 16 kHz, and
/// converts from the device's native sample format when it isn't `f32`.
pub struct AudioCapture {
    device: Device,
    config: StreamConfig,
}

impl AudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;

        let supported = device.default_input_config()?;
        tracing::info!(
            "Audio device: {:?}, sample rate: {}, channels: {}, format: {:?}",
            device.name(),
            supported.sample_rate().0,
            supported.channels(),
            supported.sample_format()
        );

        // 16 kHz mono — the format both ElevenLabs and Voxtral expect
        let config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self { device, config })
    }

    /// Start capturing audio. The returned `Stream` must be kept alive for the
    /// duration of capture — dropping it stops the audio device.
    pub fn start(&self) -> Result<(Stream, mpsc::Receiver<Vec<f32>>)> {
        let (tx, rx) = mpsc::channel::<Vec<f32>>(SAMPLE_CHANNEL_CAPACITY);

        // One query, used for rate, channel count AND sample format — the
        // format used to be read for the log line and then ignored, with the
        // stream hard-coded to `f32`. Devices that only offer integer formats
        // (common for USB interfaces, and for ALSA hw: devices on Linux) then
        // failed stream construction with an opaque backend error.
        let supported = self.device.default_input_config()?;
        let native_rate = supported.sample_rate().0;
        let native_channels = supported.channels() as usize;
        let sample_format = supported.sample_format();

        let config = if native_rate == 16000 && native_channels == 1 {
            self.config.clone()
        } else {
            // Device can't capture at 16 kHz directly — capture at native
            // rate/channels and resample + downmix in the callback
            StreamConfig {
                channels: native_channels as u16,
                sample_rate: SampleRate(native_rate),
                buffer_size: cpal::BufferSize::Default,
            }
        };

        let needs_resample = native_rate != 16000;
        let needs_downmix = native_channels > 1;
        tracing::info!(
            "Audio capture: native {}Hz {}ch {:?} → 16kHz 1ch f32 (resample={}, downmix={})",
            native_rate, native_channels, sample_format, needs_resample, needs_downmix
        );

        // Dispatch once, here, so the per-callback path stays monomorphic.
        let stream = match sample_format {
            SampleFormat::F32 => self.build_stream::<f32>(&config, tx, native_channels, native_rate),
            SampleFormat::I16 => self.build_stream::<i16>(&config, tx, native_channels, native_rate),
            SampleFormat::U16 => self.build_stream::<u16>(&config, tx, native_channels, native_rate),
            SampleFormat::I8 => self.build_stream::<i8>(&config, tx, native_channels, native_rate),
            SampleFormat::U8 => self.build_stream::<u8>(&config, tx, native_channels, native_rate),
            SampleFormat::I32 => self.build_stream::<i32>(&config, tx, native_channels, native_rate),
            SampleFormat::U32 => self.build_stream::<u32>(&config, tx, native_channels, native_rate),
            SampleFormat::F64 => self.build_stream::<f64>(&config, tx, native_channels, native_rate),
            other => anyhow::bail!("Unsupported input sample format: {:?}", other),
        }?;

        stream.play()?;
        Ok((stream, rx))
    }

    /// Build the input stream for one concrete device sample type `T`,
    /// converting to `f32` in the callback via cpal's `Sample` trait.
    fn build_stream<T>(
        &self,
        config: &StreamConfig,
        tx: mpsc::Sender<Vec<f32>>,
        native_channels: usize,
        native_rate: u32,
    ) -> Result<Stream>
    where
        T: SizedSample + 'static,
        f32: FromSample<T>,
    {
        let err_tx = tx.clone();
        let needs_resample = native_rate != 16000;
        let needs_downmix = native_channels > 1;
        let resample_ratio = if needs_resample {
            16000.0 / native_rate as f64
        } else {
            1.0
        };

        let mut resample_state = ResampleState {
            accumulator: 0.0,
            last_sample: 0.0,
        };
        let mut float_buf: Vec<f32> = Vec::new();
        let mut mono_buf: Vec<f32> = Vec::new();
        let mut out_buf: Vec<f32> = Vec::new();
        let mut dropped_chunks: u64 = 0;

        let stream = self.device.build_input_stream(
            config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                // Normalize to f32 in [-1.0, 1.0]. For T = f32 this is the
                // identity conversion and optimizes out.
                float_buf.clear();
                float_buf.extend(data.iter().map(|s| s.to_sample::<f32>()));

                let samples: &[f32] = if needs_downmix {
                    mono_buf.clear();
                    mono_buf.extend(
                        float_buf
                            .chunks(native_channels)
                            .map(|frame| frame.iter().sum::<f32>() / native_channels as f32),
                    );
                    &mono_buf
                } else {
                    &float_buf
                };

                let out: &[f32] = if needs_resample {
                    out_buf.clear();
                    resample_linear(samples, resample_ratio, &mut resample_state, &mut out_buf);
                    &out_buf
                } else {
                    samples
                };

                // Never blocks/spins: `try_send_reserving` is a `try_send`
                // gated on cheap atomic capacity()/is_closed() reads. On a
                // stalled consumer this drops the chunk instead of growing
                // memory without bound, and leaves `SENTINEL_RESERVE` slots
                // untouched so the error-callback sentinel below can never
                // be starved out by ordinary audio data. A `Closed` result
                // (consumer torn down — normal teardown) is dropped
                // silently, matching pre-branch behavior; only a genuinely
                // `Full` channel (consumer alive but stalled) is worth
                // warning about.
                match try_send_reserving(&tx, SENTINEL_RESERVE, out.to_vec()) {
                    SendOutcome::Sent => {}
                    SendOutcome::Full => warn_channel_full(&mut dropped_chunks, "Audio sample"),
                    SendOutcome::Closed => {}
                }
            },
            move |err| {
                tracing::error!("Audio capture error: {}", err);
                // Sentinel convention: an empty Vec is delivered to the
                // consumer to mark that a capture error occurred. The only
                // consumer today (`AudioPipeline::start`'s chunker thread in
                // `src/audio/mod.rs`) currently just skips empty batches
                // (`if samples.is_empty() { continue; }`) without acting on
                // the error — this delivery preserves pre-branch behavior
                // (silent drop-and-continue) rather than being unused
                // plumbing; a future consumer could still read it as an
                // explicit error signal instead of a plain empty batch.
                // `SENTINEL_RESERVE` slots are never
                // touched by the data-callback path above, so this
                // `try_send` should always succeed while a consumer is
                // still attached. If it fails with `Full`, something has
                // gone very wrong (e.g. concurrent error callbacks racing
                // each other for reserved slots) — that's rare and
                // important enough to always log, not rate-limit. A
                // `Closed` failure just means the consumer already tore
                // down (e.g. the recording session ended moments ago) —
                // nobody is left to notify, so it's dropped silently rather
                // than logged as an error.
                match err_tx.try_send(Vec::new()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::error!(
                            "Audio capture error sentinel dropped — channel full despite {} reserved slots",
                            SENTINEL_RESERVE
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                }
            },
            None,
        )?;

        Ok(stream)
    }
}

struct ResampleState {
    accumulator: f64,
    last_sample: f32,
}

/// Linear interpolation resampler. Lower quality than polyphase/sinc but
/// sufficient for speech audio and adds negligible latency per buffer.
/// State is persisted across callbacks to avoid discontinuities at buffer edges.
fn resample_linear(
    input: &[f32],
    ratio: f64,
    state: &mut ResampleState,
    output: &mut Vec<f32>,
) {
    for &sample in input {
        state.accumulator += ratio;
        while state.accumulator >= 1.0 {
            state.accumulator -= 1.0;
            let t = (state.accumulator / ratio) as f32;
            let interpolated = state.last_sample * t + sample * (1.0 - t);
            output.push(interpolated);
        }
        state.last_sample = sample;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_state() -> ResampleState {
        ResampleState {
            accumulator: 0.0,
            last_sample: 0.0,
        }
    }

    /// At 48000Hz -> 16000Hz the ratio is exactly 1/3, so the accumulator
    /// crosses 1.0 with a ~0 fractional remainder every time (the overshoot
    /// after subtracting 1.0 is exactly 0.0 in f64). With overshoot == 0,
    /// the correctly-weighted interpolation (overshoot / ratio) is also
    /// exactly 0, so the output correctly picks the current sample with no
    /// blending — the sample boundary lands exactly on an input sample, so
    /// there is nothing to interpolate between. This is the correct result
    /// for this integer-ratio edge case, not a bug: it pins the same values
    /// as before the /ratio fix (verified by hand-walking the accumulator:
    /// 1/3, 2/3, 1.0→0 remainder, repeating exactly every 3 input samples).
    #[test]
    fn resample_48k_to_16k_exact_ratio_picks_current_sample() {
        let ratio = 16000.0 / 48000.0;
        let input: Vec<f32> = (0..9).map(|i| i as f32).collect();
        let mut state = new_state();
        let mut output = Vec::new();

        resample_linear(&input, ratio, &mut state, &mut output);

        assert_eq!(output, vec![2.0, 5.0, 8.0]);
    }

    /// 44100Hz -> 16000Hz golden vector, captured from the function's actual
    /// output after the /ratio interpolation-weight fix (methodology: ran
    /// this test with a placeholder expectation, printed the real output,
    /// then pasted it back in as the pinned golden value). Each output was
    /// independently spot-checked by hand as a properly weighted blend of
    /// its two adjacent input samples (`last_sample * t + sample * (1 - t)`
    /// for the accumulator's fractional position `t` at that crossing).
    #[test]
    fn resample_44100_to_16000_ramp_golden() {
        let ratio = 16000.0 / 44100.0;
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let mut state = new_state();
        let mut output = Vec::new();

        resample_linear(&input, ratio, &mut state, &mut output);

        // Captured from actual output on 2026-07-18, after the /ratio fix
        // (see the doc comment above for the capture methodology).
        let expected: Vec<f32> = vec![
            0.0175625, 0.045125, 0.0726875, 0.10025, 0.1278125, 0.155375, 0.1829375,
            0.2105, 0.2380625, 0.265625, 0.2931875, 0.32075, 0.34831253, 0.375875,
            0.4034375, 0.431, 0.4585625, 0.486125, 0.5136875, 0.54125005, 0.5688125,
            0.596375, 0.6239375, 0.6515, 0.6790625, 0.706625, 0.7341875, 0.76175,
            0.7893125, 0.816875, 0.8444375, 0.87200004, 0.8995625, 0.927125, 0.9546875,
            0.98225,
        ];
        assert_eq!(output, expected);
    }

    /// State must persist across calls: feeding 100 samples in one call must
    /// produce the same output as feeding the same 100 samples split across
    /// two 50-sample calls sharing the same state.
    #[test]
    fn resample_state_continuity_across_calls() {
        let ratio = 16000.0 / 44100.0;
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();

        let mut state_split = new_state();
        let mut split_output = Vec::new();
        resample_linear(&input[..50], ratio, &mut state_split, &mut split_output);
        resample_linear(&input[50..], ratio, &mut state_split, &mut split_output);

        let mut state_whole = new_state();
        let mut whole_output = Vec::new();
        resample_linear(&input, ratio, &mut state_whole, &mut whole_output);

        assert_eq!(split_output, whole_output);
    }

    #[test]
    fn resample_empty_input_returns_empty_output() {
        let mut state = new_state();
        let mut output = Vec::new();
        resample_linear(&[], 16000.0 / 48000.0, &mut state, &mut output);
        assert!(output.is_empty());
    }
}

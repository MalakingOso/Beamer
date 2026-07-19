use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};
use tokio::sync::mpsc;

use super::{try_send_reserving, warn_channel_full};

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
/// exceedingly rare (not per-callback), so a small reserve is ample.
const SENTINEL_RESERVE: usize = 4;

/// Wraps cpal device setup and provides a mono 16 kHz f32 sample stream.
/// Resamples from the device's native rate when it differs from 16 kHz.
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
            "Audio device: {:?}, sample rate: {}, channels: {}",
            device.name(),
            supported.sample_rate().0,
            supported.channels()
        );

        // 16 kHz mono — the format both ElevenLabs and Voxtral expect
        let config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self { device, config })
    }

    pub fn device_sample_rate(&self) -> u32 {
        self.device
            .default_input_config()
            .map(|c| c.sample_rate().0)
            .unwrap_or(16000)
    }

    /// Start capturing audio. The returned `Stream` must be kept alive for the
    /// duration of capture — dropping it stops the audio device.
    pub fn start(&self) -> Result<(Stream, mpsc::Receiver<Vec<f32>>)> {
        let (tx, rx) = mpsc::channel::<Vec<f32>>(SAMPLE_CHANNEL_CAPACITY);
        let err_tx = tx.clone();

        let native_rate = self.device_sample_rate();
        let native_channels = self
            .device
            .default_input_config()?
            .channels() as usize;

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
            "Audio capture: native {}Hz {}ch → 16kHz 1ch (resample={}, downmix={})",
            native_rate, native_channels, needs_resample, needs_downmix
        );
        let resample_ratio = if needs_resample {
            16000.0 / native_rate as f64
        } else {
            1.0
        };

        let mut resample_state = ResampleState {
            accumulator: 0.0,
            last_sample: 0.0,
        };
        let mut mono_buf: Vec<f32> = Vec::new();
        let mut out_buf: Vec<f32> = Vec::new();
        let mut dropped_chunks: u64 = 0;

        let stream = self.device.build_input_stream(
            &config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                let samples: &[f32] = if needs_downmix {
                    mono_buf.clear();
                    mono_buf.extend(
                        data.chunks(native_channels)
                            .map(|frame| frame.iter().sum::<f32>() / native_channels as f32),
                    );
                    &mono_buf
                } else {
                    data
                };

                let out: &[f32] = if needs_resample {
                    out_buf.clear();
                    resample_linear(samples, resample_ratio, &mut resample_state, &mut out_buf);
                    &out_buf
                } else {
                    samples
                };

                // Never blocks/spins: `try_send_reserving` is a `try_send`
                // gated on a cheap atomic capacity() read. On a stalled
                // consumer this drops the chunk instead of growing memory
                // without bound, and leaves `SENTINEL_RESERVE` slots
                // untouched so the error-callback sentinel below can never
                // be starved out by ordinary audio data.
                if !try_send_reserving(&tx, SENTINEL_RESERVE, out.to_vec()) {
                    warn_channel_full(&mut dropped_chunks, "Audio sample");
                }
            },
            move |err| {
                tracing::error!("Audio capture error: {}", err);
                // Sentinel convention: an empty Vec tells the consumer a
                // capture error occurred. `SENTINEL_RESERVE` slots are never
                // touched by the data-callback path above, so this
                // `try_send` should always succeed. If it doesn't, something
                // has gone very wrong (e.g. concurrent error callbacks
                // racing each other for reserved slots) — that's rare and
                // important enough to always log, not rate-limit.
                if err_tx.try_send(Vec::new()).is_err() {
                    tracing::error!(
                        "Audio capture error sentinel dropped — channel full despite {} reserved slots",
                        SENTINEL_RESERVE
                    );
                }
            },
            None,
        )?;

        stream.play()?;
        Ok((stream, rx))
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
    /// as before the /ratio fix, verified analytically (see
    /// task-TB.4-report.md for the accumulator walk-through).
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
    /// independently verified to be a properly weighted blend of its two
    /// adjacent input samples — see task-TB.4-report.md for the
    /// hand-derivation and spot checks.
    #[test]
    fn resample_44100_to_16000_ramp_golden() {
        let ratio = 16000.0 / 44100.0;
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let mut state = new_state();
        let mut output = Vec::new();

        resample_linear(&input, ratio, &mut state, &mut output);

        // Captured from actual output on 2026-07-18, after the /ratio fix
        // (see task-TB.4-report.md for the capture methodology).
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

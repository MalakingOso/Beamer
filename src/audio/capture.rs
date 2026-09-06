use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SampleFormat, SampleRate, SizedSample, Stream, StreamConfig};
use tokio::sync::mpsc;

use super::{try_send_reserving, warn_channel_full, SendOutcome};

/// Raw-sample channel capacity, sized for the fastest realistic callback cadence
/// (5ms per callback → 200/sec) so it holds >= 60s of audio: 60 * 200 = 12_000.
pub(crate) const SAMPLE_CHANNEL_CAPACITY: usize = 12_000;

/// Slots withheld from sample data so the rare device-error sentinel (empty
/// `Vec` from the cpal error callback) always has room to `try_send`.
const SENTINEL_RESERVE: usize = 4;

/// cpal device setup producing a mono 16 kHz f32 sample stream, resampling and
/// downmixing from the device's native rate/channels when they differ.
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

        // 16 kHz mono — the format the transcription backends expect.
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

        // One query drives rate, channel count AND sample format: integer-only
        // devices (common for USB interfaces) fail stream construction if the
        // format is assumed to be `f32`.
        let supported = self.device.default_input_config()?;
        let native_rate = supported.sample_rate().0;
        let native_channels = supported.channels() as usize;
        let sample_format = supported.sample_format();

        let config = if native_rate == 16000 && native_channels == 1 {
            self.config.clone()
        } else {
            // Capture at native rate/channels; resample + downmix in the callback.
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

    /// Build the input stream for one concrete device sample type `T`.
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
                // Normalize to f32 in [-1.0, 1.0] (identity for T = f32).
                float_buf.clear();
                float_buf.extend(data.iter().map(|s| s.to_sample::<f32>()));

                let samples: &[f32] = if needs_downmix {
                    mono_buf.clear();
                    mono_buf.extend(float_buf.chunks(native_channels).map(|frame| {
                        // A trailing runt chunk is shorter than a full frame:
                        // divide by what is there, not the channel count.
                        frame.iter().sum::<f32>() / frame.len().max(1) as f32
                    }));
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

                // Never blocks: drops the chunk on a stalled consumer instead of
                // growing memory, and leaves `SENTINEL_RESERVE` slots untouched
                // for the error-callback sentinel below.
                match try_send_reserving(&tx, SENTINEL_RESERVE, out.to_vec()) {
                    SendOutcome::Sent => {}
                    SendOutcome::Full => warn_channel_full(&mut dropped_chunks, "Audio sample"),
                    SendOutcome::Closed => {}
                }
            },
            move |err| {
                tracing::error!("Audio capture error: {}", err);
                // Empty Vec marks a capture error. The consumer skips empty
                // batches; reserved slots keep this deliverable. `Full` here
                // is unexpected enough to always log; `Closed` means the
                // consumer already tore down, so drop silently.
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

/// Linear interpolation resampler, sufficient for speech. State persists across
/// callbacks to avoid discontinuities at buffer edges.
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

    /// At 48000 → 16000 the ratio is exactly 1/3, so each output lands exactly
    /// on an input sample with nothing to blend — picking it unblended is
    /// correct here, not a bug.
    #[test]
    fn resample_48k_to_16k_exact_ratio_picks_current_sample() {
        let ratio = 16000.0 / 48000.0;
        let input: Vec<f32> = (0..9).map(|i| i as f32).collect();
        let mut state = new_state();
        let mut output = Vec::new();

        resample_linear(&input, ratio, &mut state, &mut output);

        assert_eq!(output, vec![2.0, 5.0, 8.0]);
    }

    /// 44100 → 16000 golden vector pinning the interpolation weights.
    #[test]
    fn resample_44100_to_16000_ramp_golden() {
        let ratio = 16000.0 / 44100.0;
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let mut state = new_state();
        let mut output = Vec::new();

        resample_linear(&input, ratio, &mut state, &mut output);

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

    /// Split feeds sharing one state must match a single whole feed.
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

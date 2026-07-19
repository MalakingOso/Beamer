use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};
use tokio::sync::mpsc;

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
    pub fn start(&self) -> Result<(Stream, mpsc::UnboundedReceiver<Vec<f32>>)> {
        let (tx, rx) = mpsc::unbounded_channel::<Vec<f32>>();
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

                let _ = tx.send(out.to_vec());
            },
            move |err| {
                tracing::error!("Audio capture error: {}", err);
                let _ = err_tx.send(Vec::new());
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
            let t = state.accumulator as f32;
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
    /// crosses 1.0 with a ~0 fractional remainder every time. This makes the
    /// "interpolation" degenerate into picking every 3rd input sample
    /// (a known bug — later tasks fix this). This test pins that CURRENT
    /// behavior so a future refactor doesn't silently change it.
    #[test]
    fn resample_48k_to_16k_is_pure_decimation() {
        let ratio = 16000.0 / 48000.0;
        let input: Vec<f32> = (0..9).map(|i| i as f32).collect();
        let mut state = new_state();
        let mut output = Vec::new();

        resample_linear(&input, ratio, &mut state, &mut output);

        assert_eq!(output, vec![2.0, 5.0, 8.0]);
    }

    /// 44100Hz -> 16000Hz golden vector, captured from the function's actual
    /// current output (methodology: ran this test with a placeholder
    /// expectation, printed the real output, then pasted it back in as the
    /// pinned golden value).
    #[test]
    fn resample_44100_to_16000_ramp_golden() {
        let ratio = 16000.0 / 44100.0;
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let mut state = new_state();
        let mut output = Vec::new();

        resample_linear(&input, ratio, &mut state, &mut output);

        // Captured from actual output on 2026-07-18 (see task-T0.1-report.md
        // for the capture methodology).
        let expected: Vec<f32> = vec![
            0.019115647, 0.048231293, 0.077346936, 0.10646258, 0.12920634, 0.158322,
            0.18743765, 0.21655329, 0.23929705, 0.2684127, 0.29752836, 0.326644,
            0.34938776, 0.37850338, 0.40761906, 0.4367347, 0.45947847, 0.4885941,
            0.51770973, 0.5468254, 0.5695692, 0.59868485, 0.62780046, 0.6569161,
            0.67965984, 0.70877546, 0.73789114, 0.76700675, 0.78975064, 0.8188662,
            0.8479819, 0.8770975, 0.89984125, 0.9289569, 0.95807254, 0.9871882,
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

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};
use std::sync::Arc;
use tokio::sync::mpsc;

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

        let config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self { device, config })
    }

    pub fn device_sample_rate(&self) -> u32 {
        let supported = self.device.default_input_config().unwrap();
        supported.sample_rate().0
    }

    /// Start capturing audio. Returns a stream handle (must be kept alive) and a receiver of f32 samples.
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
            // Use native config and resample in callback
            StreamConfig {
                channels: native_channels as u16,
                sample_rate: SampleRate(native_rate),
                buffer_size: cpal::BufferSize::Default,
            }
        };

        let needs_resample = native_rate != 16000;
        let needs_downmix = native_channels > 1;
        let resample_ratio = if needs_resample {
            16000.0 / native_rate as f64
        } else {
            1.0
        };

        let resample_state = Arc::new(std::sync::Mutex::new(ResampleState {
            accumulator: 0.0,
            last_sample: 0.0,
        }));

        let stream = self.device.build_input_stream(
            &config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                let mut samples: Vec<f32> = if needs_downmix {
                    // Downmix to mono by averaging channels
                    data.chunks(native_channels)
                        .map(|frame| frame.iter().sum::<f32>() / native_channels as f32)
                        .collect()
                } else {
                    data.to_vec()
                };

                if needs_resample {
                    samples = resample_linear(&samples, resample_ratio, &resample_state);
                }

                let _ = tx.send(samples);
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

/// Simple linear interpolation resampler
fn resample_linear(
    input: &[f32],
    ratio: f64,
    state: &Arc<std::sync::Mutex<ResampleState>>,
) -> Vec<f32> {
    let mut state = state.lock().unwrap();
    let mut output = Vec::with_capacity((input.len() as f64 * ratio) as usize + 1);

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

    output
}

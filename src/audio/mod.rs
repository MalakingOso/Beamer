pub mod capture;
pub mod vad;

use anyhow::Result;
use cpal::Stream;
use std::io::Cursor;
use tokio::sync::mpsc;

use self::capture::AudioCapture;
use self::vad::{VadEvent, VadProcessor};

pub enum AudioEvent {
    SpeechStart,
    SpeechEnd,
    AudioReady(Vec<u8>),
    AudioChunk(Vec<u8>),
}

pub struct AudioPipeline {
    capture: AudioCapture,
}

impl AudioPipeline {
    pub fn new() -> Result<Self> {
        Ok(Self {
            capture: AudioCapture::new()?,
        })
    }

    /// Start the audio pipeline. Returns a handle to keep alive and a receiver of AudioEvents.
    pub fn start(
        &self,
        vad_aggressiveness: u8,
        pre_buffer_ms: u32,
        silence_timeout_ms: u32,
    ) -> Result<(Stream, mpsc::UnboundedReceiver<AudioEvent>)> {
        let (stream, mut sample_rx) = self.capture.start()?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // VAD uses FFI pointers that aren't Send, so process on a dedicated thread
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let mut vad = VadProcessor::new(vad_aggressiveness, pre_buffer_ms, silence_timeout_ms);
            let mut speech_buffer: Vec<i16> = Vec::new();

            loop {
                let samples = match rt.block_on(sample_rx.recv()) {
                    Some(s) => s,
                    None => break,
                };

                if samples.is_empty() {
                    continue;
                }

                let (events, speech_samples) = vad.process(&samples);

                if !speech_samples.is_empty() {
                    speech_buffer.extend_from_slice(&speech_samples);

                    let chunk_bytes: Vec<u8> = speech_samples
                        .iter()
                        .flat_map(|&s| s.to_le_bytes())
                        .collect();
                    let _ = event_tx.send(AudioEvent::AudioChunk(chunk_bytes));
                }

                for event in events {
                    match event {
                        VadEvent::SpeechStart => {
                            speech_buffer.clear();
                            let _ = event_tx.send(AudioEvent::SpeechStart);
                        }
                        VadEvent::SpeechEnd => {
                            if let Ok(wav) = encode_wav(&speech_buffer) {
                                let _ = event_tx.send(AudioEvent::AudioReady(wav));
                            }
                            speech_buffer.clear();
                            let _ = event_tx.send(AudioEvent::SpeechEnd);
                        }
                    }
                }
            }
        });

        Ok((stream, event_rx))
    }
}

fn encode_wav(samples: &[i16]) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(cursor.into_inner())
}

use webrtc_vad::Vad;

const FRAME_SIZE: usize = 320; // 20ms at 16kHz
const SPEECH_FRAMES_THRESHOLD: usize = 3;

pub enum VadEvent {
    SpeechStart,
    SpeechEnd,
}

pub struct VadProcessor {
    vad: Vad,
    state: VadState,
    speech_frame_count: usize,
    silence_frame_count: usize,
    silence_threshold: usize,
    frame_buffer: Vec<i16>,
    pre_buffer: Vec<Vec<i16>>,
    pre_buffer_frames: usize,
}

#[derive(PartialEq)]
enum VadState {
    Idle,
    Speaking,
}

impl VadProcessor {
    pub fn new(aggressiveness: u8, pre_buffer_ms: u32, silence_timeout_ms: u32) -> Self {
        let mut vad = Vad::new();
        let mode = match aggressiveness {
            1 => webrtc_vad::VadMode::Quality,
            3 => webrtc_vad::VadMode::VeryAggressive,
            _ => webrtc_vad::VadMode::Aggressive,
        };
        vad.set_mode(mode);

        let pre_buffer_frames = (pre_buffer_ms as usize) / 20; // 20ms per frame
        let silence_threshold = (silence_timeout_ms as usize) / 20; // 20ms per frame

        Self {
            vad,
            state: VadState::Idle,
            speech_frame_count: 0,
            silence_frame_count: 0,
            silence_threshold,
            frame_buffer: Vec::with_capacity(FRAME_SIZE),
            pre_buffer: Vec::with_capacity(pre_buffer_frames + 1),
            pre_buffer_frames,
        }
    }

    /// Feed f32 samples (16kHz mono) and get VAD events
    pub fn process(&mut self, samples: &[f32]) -> (Vec<VadEvent>, Vec<i16>) {
        let mut events = Vec::new();
        let mut speech_samples = Vec::new();

        // Convert f32 to i16
        let i16_samples: Vec<i16> = samples
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();

        for &sample in &i16_samples {
            self.frame_buffer.push(sample);

            if self.frame_buffer.len() >= FRAME_SIZE {
                let frame: Vec<i16> = self.frame_buffer.drain(..FRAME_SIZE).collect();
                let is_speech = self.vad.is_voice_segment(&frame).unwrap_or(false);

                match self.state {
                    VadState::Idle => {
                        // Maintain pre-buffer
                        self.pre_buffer.push(frame.clone());
                        if self.pre_buffer.len() > self.pre_buffer_frames {
                            self.pre_buffer.remove(0);
                        }

                        if is_speech {
                            self.speech_frame_count += 1;
                            if self.speech_frame_count >= SPEECH_FRAMES_THRESHOLD {
                                self.state = VadState::Speaking;
                                self.silence_frame_count = 0;
                                events.push(VadEvent::SpeechStart);

                                // Flush pre-buffer into speech samples
                                for pre_frame in self.pre_buffer.drain(..) {
                                    speech_samples.extend_from_slice(&pre_frame);
                                }
                                speech_samples.extend_from_slice(&frame);
                            }
                        } else {
                            self.speech_frame_count = 0;
                        }
                    }
                    VadState::Speaking => {
                        speech_samples.extend_from_slice(&frame);

                        if !is_speech {
                            self.silence_frame_count += 1;
                            if self.silence_frame_count >= self.silence_threshold {
                                self.state = VadState::Idle;
                                self.speech_frame_count = 0;
                                self.silence_frame_count = 0;
                                events.push(VadEvent::SpeechEnd);
                            }
                        } else {
                            self.silence_frame_count = 0;
                        }
                    }
                }
            }
        }

        (events, speech_samples)
    }
}

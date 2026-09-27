use anyhow::Result;
use dioxus::prelude::*;
use futures_util::StreamExt;

use crate::audio::AudioPipeline;
use crate::config::Config;
use crate::hotkey::{CaptureMode, HotkeyEvent};
use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::transcription;
use crate::ui::history::TranscriptionHistory;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

mod notify;
mod session;
mod sink;
use notify::show_notification;
use session::{buffer_tail_audio, StopReason};

/// Recording lifecycle state, drives both the pill overlay and home-page status dot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RecordingState {
    #[default]
    Idle,
    Recording,
    Processing,
}

/// Central orchestration loop: hotkey events → audio capture → transcription → text injection.
/// Runs as a Dioxus coroutine, receiving `HotkeyEvent`s and driving recording sessions.
pub async fn run(
    mut hotkey_rx: UnboundedReceiver<HotkeyEvent>,
    config: Signal<Config>,
    mut rec_state: Signal<RecordingState>,
    mut last_injection: Signal<String>,
    mut history: Signal<TranscriptionHistory>,
    mut status_log: Signal<StatusLog>,
    mut notes: Signal<NoteStore>,
    mut tasks: Signal<TaskStore>,
    mut active_mode: Signal<CaptureMode>,
    note_passes: Coroutine<PipelineRequest>,
) {
    tracing::info!("Orchestrator started, waiting for hotkey events");
    log_status(&mut status_log, LogLevel::Info, "Orchestrator ready");

    while let Some(event) = hotkey_rx.next().await {
        match event {
            HotkeyEvent::RecordStart(capture_mode) => {
                // Set before `handle_recording` sets `Recording`, or the pill
                // flashes the wrong style for one frame.
                active_mode.set(capture_mode);
                if let Err(e) = handle_recording(
                    &config,
                    &mut rec_state,
                    &mut last_injection,
                    &mut history,
                    &mut hotkey_rx,
                    &mut status_log,
                    &mut notes,
                    &mut tasks,
                    capture_mode,
                    note_passes,
                )
                .await
                {
                    tracing::error!("Recording session error: {}", e);
                    log_status(&mut status_log, LogLevel::Error, format!("Recording error: {}", e));
                    show_notification("Beamer", &format!("Recording error: {}", e));
                }
                rec_state.set(RecordingState::Idle);
            }
            HotkeyEvent::RecordStop => {}
        }
    }
}

/// Drive one recording session: connect WebSocket → capture mic → stream audio → inject text.
/// On stop, sends a commit signal and drains final transcripts before returning.
async fn handle_recording(
    config: &Signal<Config>,
    rec_state: &mut Signal<RecordingState>,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    hotkey_rx: &mut UnboundedReceiver<HotkeyEvent>,
    status_log: &mut Signal<StatusLog>,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    capture_mode: CaptureMode,
    note_passes: Coroutine<PipelineRequest>,
) -> Result<()> {
    let cfg = config.read().clone();
    let backend = &cfg.transcription.backend;
    let language = &cfg.transcription.language;
    let backends = cfg.injection.backends.clone();

    // Exhaustive on purpose: an unknown backend must error, never silently
    // fall back to another model.
    let (key_name, display_name) = match backend.as_str() {
        "voxtral_batch" => ("mistral_api_key", "Voxtral"),
        "elevenlabs_batch" | "elevenlabs_medical_batch" => ("elevenlabs_api_key", "ElevenLabs"),
        other => {
            tracing::error!("Unknown transcription backend '{}'", other);
            log_status(
                status_log,
                LogLevel::Error,
                format!("Unknown transcription backend '{other}' — check Settings"),
            );
            show_notification(
                "Beamer",
                &format!("Unknown transcription backend '{other}'. Open Settings to pick one."),
            );
            return Ok(());
        }
    };
    let api_key = crate::config::load_api_key(key_name);
    if api_key.is_empty() {
        tracing::error!("No {} API key found in keyring (looked up '{}'). Open Settings to add one.", display_name, key_name);
        log_status(status_log, LogLevel::Error, format!("No {} API key configured — open Settings", display_name));
        show_notification("Beamer", &format!("No {} API key configured. Open Settings to add one.", display_name));
        return Ok(());
    }

    handle_batch_recording(
        backend, &api_key, language, &backends, &cfg, config,
        rec_state, last_injection, history, hotkey_rx, status_log,
        notes, tasks, capture_mode, note_passes,
    )
    .await
}

/// Drive one batch recording session: capture mic → buffer all PCM → POST to the batch API.
async fn handle_batch_recording(
    backend: &str,
    api_key: &str,
    language: &str,
    backends: &[String],
    cfg: &Config,
    config: &Signal<Config>,
    rec_state: &mut Signal<RecordingState>,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    hotkey_rx: &mut UnboundedReceiver<HotkeyEvent>,
    status_log: &mut Signal<StatusLog>,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    capture_mode: CaptureMode,
    note_passes: Coroutine<PipelineRequest>,
) -> Result<()> {
    let pipeline = match AudioPipeline::new() {
        Ok(p) => p,
        Err(e) => {
            log_status(status_log, LogLevel::Error, "Microphone not available");
            show_notification("Beamer", "Microphone not available");
            return Err(e);
        }
    };
    let (_stream, mut audio_rx) = pipeline.start()?;

    rec_state.set(RecordingState::Recording);
    log_status(status_log, LogLevel::Info, "Recording started");
    // Guard resumes playback on drop, on every exit path below.
    let media_pause = if cfg.recording.pause_media {
        crate::media::pause_media_if_playing()
    } else {
        None
    };
    crate::sounds::play_start_sound();

    let mut pcm_buffer: Vec<u8> = Vec::new();
    let mut stop_reason = StopReason::UserStop;
    loop {
        tokio::select! {
            hotkey_event = hotkey_rx.next() => {
                match hotkey_event {
                    Some(HotkeyEvent::RecordStop) | None => break,
                    Some(HotkeyEvent::RecordStart(_)) => {}
                }
            }
            chunk = audio_rx.recv() => {
                match chunk {
                    Some(bytes) if !bytes.is_empty() => {
                        pcm_buffer.extend_from_slice(&bytes);
                    }
                    _ => {
                        log_status(status_log, LogLevel::Warn, "Audio channel closed");
                        stop_reason = StopReason::AudioLost;
                        break;
                    }
                }
            }
        }
    }

    // Shared teardown for both exit reasons.
    crate::sounds::play_stop_sound();
    rec_state.set(RecordingState::Processing);
    drop(media_pause);

    // Skip tail capture when the audio channel died: `recv()` on a closed
    // channel returns immediately and would spin hot until the deadline.
    if stop_reason == StopReason::UserStop {
        buffer_tail_audio(&mut audio_rx, &mut pcm_buffer).await;
    }

    if pcm_buffer.is_empty() {
        log_status(status_log, LogLevel::Info, "No audio captured");
        return Ok(());
    }

    // Warn if the buffer is all silence (wrong device or muted mic).
    let (max_amplitude, rms) = {
        let mut peak: u16 = 0;
        let mut sum_sq: f64 = 0.0;
        let count = pcm_buffer.len() / 2;

        for chunk in pcm_buffer.chunks_exact(2) {
            let s_i16 = i16::from_le_bytes([chunk[0], chunk[1]]);
            peak = peak.max(s_i16.unsigned_abs());
            let s = s_i16 as f64;
            sum_sq += s * s;
        }

        (peak, (sum_sq / count as f64).sqrt())
    };
    tracing::info!("Audio stats: {:.1}s, peak={}, RMS={:.0}", pcm_buffer.len() as f64 / 32000.0, max_amplitude, rms);
    if max_amplitude < 100 {
        log_status(status_log, LogLevel::Warn,
            "Audio appears to be silence — check that your microphone is working and selected as the default input device");
        tracing::warn!("Audio buffer is essentially silence (peak={}). Wrong input device or mic muted?", max_amplitude);
    }

    let audio_secs = pcm_buffer.len() as f64 / (16000.0 * 2.0);
    let backend_label = if backend == "voxtral_batch" { "Voxtral" } else { "ElevenLabs" };
    log_status(
        status_log,
        LogLevel::Info,
        format!("Sending {:.1}s of audio to {}...", audio_secs, backend_label),
    );

    let start = tokio::time::Instant::now();
    let vocab = crate::config::vocabulary::Vocabulary::load()?.list().to_vec();
    // Exhaustive on purpose (see `handle_recording`): a new backend must land
    // here, never silently fall back to another model.
    let result = match backend {
        "voxtral_batch" => {
            transcription::transcribe_voxtral_batch(api_key, pcm_buffer, &vocab).await
        }
        "elevenlabs_medical_batch" => {
            transcription::transcribe_medical_batch(
                api_key,
                pcm_buffer,
                language,
                &vocab,
                cfg.transcription.no_verbatim,
            )
            .await
        }
        "elevenlabs_batch" => {
            transcription::transcribe_batch(
                api_key,
                pcm_buffer,
                language,
                &vocab,
                cfg.transcription.no_verbatim,
            )
            .await
        }
        // Unreachable: `handle_recording` rejects unknown backends before we
        // get here. Spelled out rather than `_ =>` so a new backend fails
        // loudly instead of silently becoming Scribe v2.
        other => Err(anyhow::anyhow!("'{other}' is not a batch backend")),
    };
    match result {
        Ok(text) => {
            let elapsed = start.elapsed();
            log_status(
                status_log,
                LogLevel::Info,
                format!("[batch] {:.1}s round-trip: {}", elapsed.as_secs_f64(), text),
            );
            if !text.trim().is_empty() {
                sink::deliver(&text, capture_mode, backends, &cfg.injection.paste_shortcut,
                        last_injection, history, status_log, notes, tasks, config,
                        note_passes).await;
            }
        }
        Err(e) => {
            tracing::error!("Batch transcription failed: {}", e);
            log_status(status_log, LogLevel::Error, format!("Batch transcription failed: {}", e));
            show_notification("Beamer", &format!("Transcription failed: {}", e));
        }
    }

    log_status(status_log, LogLevel::Info, "Recording stopped");
    Ok(())
}

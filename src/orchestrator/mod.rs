//! The conductor of a dictation: hotkey → **orchestrator** → audio →
//! transcription → injection (or a sticky note).
//!
//! `run` is a Dioxus coroutine fed `HotkeyEvent`s by `hotkey/`. It drives two
//! loops side by side in one task:
//! 1. Capture (`capture_loop`): `RecordStart(mode)` → check the backend and
//!    its API key (missing key → notification, no recording) → open the mic
//!    (`audio/`), set `Recording`, pause media, play the start sound, and
//!    append PCM chunks to one buffer until `RecordStop` (or a dead mic) →
//!    stop sound, media resumes, ~400 ms of tail audio (`session`) → the mic
//!    closes and the recording is queued as a `transcribe::Job`.
//! 2. Transcription (`transcribe::worker`): POSTs each job to its
//!    `transcription/` backend, in order, and `sink::deliver`s the text.
//!
//! The hotkey channel is read the whole time, so a slow upload never blocks
//! the next recording. State: `Recording` while capturing, else `Processing`
//! while any job is queued, else `Idle`.

use std::cell::Cell;

use dioxus::prelude::*;
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::audio::AudioPipeline;
use crate::config::Config;
use crate::hotkey::{CaptureMode, HotkeyEvent};
use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::ui::history::TranscriptionHistory;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

pub(crate) mod notify;
mod session;
mod sink;
mod transcribe;
use notify::show_notification;
use session::{buffer_tail_audio, StopReason};
use transcribe::Job;

/// Recording lifecycle state; drives the pill overlay, the home-page status dot
/// and (on Linux) the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RecordingState {
    #[default]
    Idle,
    Recording,
    Processing,
}

/// Leave `Recording` alone (a capture is live); otherwise `Processing` while
/// any job is queued, else `Idle`.
fn settle(rec_state: &mut Signal<RecordingState>, pending_jobs: usize) {
    if *rec_state.peek() != RecordingState::Recording {
        rec_state.set(if pending_jobs > 0 { RecordingState::Processing } else { RecordingState::Idle });
    }
}

/// The orchestrator coroutine: capture and transcription until the hotkey
/// channel closes. Session errors are logged and notified, never fatal.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    hotkey_rx: UnboundedReceiver<HotkeyEvent>,
    config: Signal<Config>,
    rec_state: Signal<RecordingState>,
    last_injection: Signal<String>,
    history: Signal<TranscriptionHistory>,
    mut status_log: Signal<StatusLog>,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    active_mode: Signal<CaptureMode>,
    note_passes: Coroutine<PipelineRequest>,
) {
    tracing::info!("Orchestrator started, waiting for hotkey events");
    log_status(&mut status_log, LogLevel::Info, "Orchestrator ready");

    // Both loops live in this one task, so a plain `Cell` is enough.
    let pending_jobs = Cell::new(0usize);
    let (job_tx, job_rx) = tokio::sync::mpsc::unbounded_channel();
    futures_util::future::join(
        capture_loop(hotkey_rx, config, rec_state, status_log, active_mode, job_tx, &pending_jobs),
        transcribe::worker(
            job_rx, config, rec_state, last_injection, history, status_log, notes, tasks,
            note_passes, &pending_jobs,
        ),
    )
    .await;
}

/// One capture per `RecordStart`, each queued for the transcription worker.
/// Returning drops `jobs`, which lets the worker finish and exit too.
async fn capture_loop(
    mut hotkey_rx: UnboundedReceiver<HotkeyEvent>,
    config: Signal<Config>,
    mut rec_state: Signal<RecordingState>,
    mut status_log: Signal<StatusLog>,
    mut active_mode: Signal<CaptureMode>,
    jobs: UnboundedSender<Job>,
    pending_jobs: &Cell<usize>,
) {
    while let Some(event) = hotkey_rx.next().await {
        // A stop with no capture running (e.g. a toggle's second press after
        // a failed start) has nothing to end.
        let HotkeyEvent::RecordStart(capture_mode) = event else { continue };
        // Set before `record` sets `Recording`, or the pill flashes the wrong
        // style for one frame.
        active_mode.set(capture_mode);
        if let Some(job) =
            record(&config, &mut rec_state, &mut hotkey_rx, &mut status_log, capture_mode).await
        {
            pending_jobs.set(pending_jobs.get() + 1);
            let _ = jobs.send(job);
        }
        settle(&mut rec_state, pending_jobs.get());
    }
    tracing::warn!("Hotkey channel closed: no further recordings");
}

/// Credential name and display name for a transcription backend. Exhaustive on
/// purpose: an unknown backend must error, never silently fall back to another
/// model.
fn backend_key(backend: &str) -> Option<(&'static str, &'static str)> {
    match backend {
        "voxtral_batch" => Some(("mistral_api_key", "Voxtral")),
        "elevenlabs_batch" | "elevenlabs_medical_batch" => Some(("elevenlabs_api_key", "ElevenLabs")),
        _ => None,
    }
}

/// Validate the backend and key, then capture one recording. `None` when
/// nothing was recorded; every such problem has already been reported.
async fn record(
    config: &Signal<Config>,
    rec_state: &mut Signal<RecordingState>,
    hotkey_rx: &mut UnboundedReceiver<HotkeyEvent>,
    status_log: &mut Signal<StatusLog>,
    capture_mode: CaptureMode,
) -> Option<Job> {
    let cfg = config.read().clone();
    let backend = cfg.transcription.backend.as_str();
    let Some((key_name, display_name)) = backend_key(backend) else {
        tracing::error!("Unknown transcription backend '{}'", backend);
        log_status(
            status_log,
            LogLevel::Error,
            format!("Unknown transcription backend '{backend}' — check Settings"),
        );
        show_notification(
            "Beamer",
            &format!("Unknown transcription backend '{backend}'. Open Settings to pick one."),
        );
        return None;
    };
    let api_key = crate::config::load_api_key(key_name);
    if api_key.is_empty() {
        tracing::error!("No {} API key found in keyring (looked up '{}'). Open Settings to add one.", display_name, key_name);
        log_status(status_log, LogLevel::Error, format!("No {} API key configured — open Settings", display_name));
        show_notification("Beamer", &format!("No {} API key configured. Open Settings to add one.", display_name));
        return None;
    }

    let started = AudioPipeline::new().and_then(|pipeline| pipeline.start());
    let (stream, mut audio_rx) = match started {
        Ok(started) => started,
        Err(e) => {
            tracing::error!("Microphone not available: {e:#}");
            log_status(status_log, LogLevel::Error, format!("Microphone not available: {e}"));
            show_notification("Beamer", "Microphone not available");
            return None;
        }
    };

    rec_state.set(RecordingState::Recording);
    log_status(status_log, LogLevel::Info, "Recording started");
    // Guard resumes playback on drop, on every exit path below.
    let media_pause = if cfg.recording.pause_media {
        crate::media::pause_media_if_playing().await
    } else {
        None
    };
    crate::sounds::play_start_sound();

    let mut pcm: Vec<u8> = Vec::new();
    let mut stop_reason = StopReason::UserStop;
    let mut mic_error_reported = false;
    // The other hotkey pressed mid-capture: its stop must not end this one,
    // so capture ends once every start has been matched by a stop.
    let mut nested_starts = 0u32;
    loop {
        tokio::select! {
            hotkey_event = hotkey_rx.next() => match hotkey_event {
                Some(HotkeyEvent::RecordStart(_)) => nested_starts += 1,
                Some(HotkeyEvent::RecordStop) if nested_starts > 0 => nested_starts -= 1,
                Some(HotkeyEvent::RecordStop) | None => break,
            },
            chunk = audio_rx.recv() => match chunk {
                // The chunker's marker for a cpal stream error.
                Some(bytes) if bytes.is_empty() => {
                    if !mic_error_reported {
                        mic_error_reported = true;
                        log_status(status_log, LogLevel::Warn,
                            "Microphone reported an error — this recording may be incomplete");
                    }
                }
                Some(bytes) => pcm.extend_from_slice(&bytes),
                None => {
                    log_status(status_log, LogLevel::Warn, "Audio channel closed");
                    stop_reason = StopReason::AudioLost;
                    break;
                }
            },
        }
    }

    // Shared teardown for both exit reasons.
    crate::sounds::play_stop_sound();
    rec_state.set(RecordingState::Processing);
    drop(media_pause);

    // Skip tail capture when the audio channel died: `recv()` on a closed
    // channel returns immediately and would spin hot until the deadline.
    if stop_reason == StopReason::UserStop {
        buffer_tail_audio(&mut audio_rx, &mut pcm).await;
    }
    // Close the mic now, not after the upload.
    drop(stream);
    log_status(status_log, LogLevel::Info, "Recording stopped");

    if pcm.is_empty() {
        log_status(status_log, LogLevel::Info, "No audio captured");
        return None;
    }
    Some(Job { pcm, api_key, cfg, capture_mode })
}

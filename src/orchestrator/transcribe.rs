//! The back half of a dictation: POST a finished recording to its backend and
//! hand the text to `sink::deliver`. Runs beside the capture loop (see
//! `run`), one job at a time, so transcripts land in the order they were
//! spoken while the next recording is already under way.

use std::cell::Cell;

use anyhow::Result;
use dioxus::prelude::*;
use tokio::sync::mpsc::UnboundedReceiver;

use super::notify::show_notification;
use super::{settle, sink, RecordingState};
use crate::config::Config;
use crate::hotkey::CaptureMode;
use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::transcription;
use crate::ui::history::TranscriptionHistory;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// One finished recording: 16 kHz mono 16-bit LE PCM plus the settings it was
/// recorded under (a Settings edit mid-upload applies to the next one).
pub(super) struct Job {
    pub pcm: Vec<u8>,
    pub api_key: String,
    pub cfg: Config,
    pub capture_mode: CaptureMode,
}

/// Transcribe and deliver queued jobs until the capture loop hangs up.
#[allow(clippy::too_many_arguments)]
pub(super) async fn worker(
    mut jobs: UnboundedReceiver<Job>,
    config: Signal<Config>,
    mut rec_state: Signal<RecordingState>,
    mut last_injection: Signal<String>,
    mut history: Signal<TranscriptionHistory>,
    mut status_log: Signal<StatusLog>,
    mut notes: Signal<NoteStore>,
    mut tasks: Signal<TaskStore>,
    note_passes: Coroutine<PipelineRequest>,
    pending: &Cell<usize>,
) {
    while let Some(job) = jobs.recv().await {
        let result = transcribe_and_deliver(
            job, &config, &mut last_injection, &mut history, &mut status_log,
            &mut notes, &mut tasks, note_passes,
        )
        .await;
        if let Err(e) = result {
            tracing::error!("Transcription error: {e:#}");
            log_status(&mut status_log, LogLevel::Error, format!("Transcription error: {e}"));
            show_notification("Beamer", &format!("Transcription error: {e}"));
        }
        pending.set(pending.get().saturating_sub(1));
        settle(&mut rec_state, pending.get());
    }
}

#[allow(clippy::too_many_arguments)]
async fn transcribe_and_deliver(
    job: Job,
    config: &Signal<Config>,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    status_log: &mut Signal<StatusLog>,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    note_passes: Coroutine<PipelineRequest>,
) -> Result<()> {
    let Job { pcm, api_key, cfg, capture_mode } = job;
    let backend = cfg.transcription.backend.as_str();
    let language = cfg.transcription.language.as_str();

    warn_if_silent(&pcm, status_log);

    let audio_secs = pcm.len() as f64 / (16000.0 * 2.0);
    let backend_label = if backend == "voxtral_batch" { "Voxtral" } else { "ElevenLabs" };
    log_status(
        status_log,
        LogLevel::Info,
        format!("Sending {:.1}s of audio to {}...", audio_secs, backend_label),
    );

    let start = tokio::time::Instant::now();
    let vocab = crate::config::vocabulary::Vocabulary::load()?.list().to_vec();
    // Exhaustive on purpose (see `backend_key`): a new backend must land
    // here, never silently fall back to another model.
    let result = match backend {
        "voxtral_batch" => transcription::transcribe_voxtral_batch(&api_key, pcm, &vocab).await,
        "elevenlabs_medical_batch" => {
            transcription::transcribe_medical_batch(
                &api_key, pcm, language, &vocab, cfg.transcription.no_verbatim,
            )
            .await
        }
        "elevenlabs_batch" => {
            transcription::transcribe_batch(
                &api_key, pcm, language, &vocab, cfg.transcription.no_verbatim,
            )
            .await
        }
        // Unreachable: capture rejects unknown backends before queueing. Spelled
        // out rather than `_ =>` so a new backend fails loudly instead of
        // silently becoming Scribe v2.
        other => Err(anyhow::anyhow!("'{other}' is not a batch backend")),
    };
    match result {
        Ok(text) => {
            log_status(
                status_log,
                LogLevel::Info,
                format!("[batch] {:.1}s round-trip: {}", start.elapsed().as_secs_f64(), text),
            );
            if !text.trim().is_empty() {
                sink::deliver(
                    &text, capture_mode, &cfg.injection.backends, &cfg.injection.paste_shortcut,
                    last_injection, history, status_log, notes, tasks, config, note_passes,
                )
                .await;
            }
        }
        Err(e) => {
            tracing::error!("Batch transcription failed: {}", e);
            log_status(status_log, LogLevel::Error, format!("Batch transcription failed: {}", e));
            show_notification("Beamer", &format!("Transcription failed: {}", e));
        }
    }
    Ok(())
}

/// Warn if the buffer is all silence (wrong device or muted mic).
fn warn_if_silent(pcm: &[u8], status_log: &mut Signal<StatusLog>) {
    let mut peak: u16 = 0;
    let mut sum_sq: f64 = 0.0;
    for sample in pcm.chunks_exact(2) {
        let s = i16::from_le_bytes([sample[0], sample[1]]);
        peak = peak.max(s.unsigned_abs());
        sum_sq += f64::from(s) * f64::from(s);
    }
    let rms = (sum_sq / (pcm.len() / 2).max(1) as f64).sqrt();
    tracing::info!("Audio stats: {:.1}s, peak={}, RMS={:.0}", pcm.len() as f64 / 32000.0, peak, rms);
    if peak < 100 {
        log_status(status_log, LogLevel::Warn,
            "Audio appears to be silence — check that your microphone is working and selected as the default input device");
        tracing::warn!("Audio buffer is essentially silence (peak={}). Wrong input device or mic muted?", peak);
    }
}

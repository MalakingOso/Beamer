use anyhow::Result;
use dioxus::prelude::*;
use futures_util::StreamExt;

use crate::audio::{try_send_reserving, warn_channel_full, AudioPipeline, SendOutcome};
use crate::config::Config;
use crate::hotkey::{CaptureMode, HotkeyEvent};
use crate::notes::pipeline::PipelineRequest;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::transcription::{self, TranscriptKind};
use crate::ui::history::TranscriptionHistory;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

mod notify;
mod session;
mod sink;
use notify::show_notification;
use session::{
    buffer_tail_audio, proves_session_live, send_commit_sentinel, stream_tail_audio, ClosedStream,
    StopReason, FINAL_TRANSCRIPT_TIMEOUT_MS,
};

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
    let paste_shortcut = cfg.injection.paste_shortcut.clone();

    // Exhaustive on purpose: an unknown backend must error, never silently
    // become ElevenLabs realtime.
    let (key_name, display_name) = match backend.as_str() {
        "voxtral" | "voxtral_batch" => ("mistral_api_key", "Voxtral"),
        "elevenlabs" | "elevenlabs_batch" => ("elevenlabs_api_key", "ElevenLabs"),
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

    if backend == "elevenlabs_batch" || backend == "voxtral_batch" {
        return handle_batch_recording(
            backend, &api_key, language, &backends, &cfg, config,
            rec_state, last_injection, history, hotkey_rx, status_log,
            notes, tasks, capture_mode, note_passes,
        ).await;
    }

    log_status(status_log, LogLevel::Info, format!("Connecting to {} realtime...", display_name));
    // A missing vocabulary file costs keyterms, never the recording.
    let vocab = crate::config::vocabulary::Vocabulary::load()
        .map(|v| v.list().to_vec())
        .unwrap_or_else(|e| {
            tracing::warn!("Could not load vocabulary, continuing without keyterms: {}", e);
            Vec::new()
        });
    let session_result = match backend.as_str() {
        "voxtral" => transcription::start_voxtral_session(&api_key).await,
        "elevenlabs" => {
            transcription::start_elevenlabs_session(
                &api_key,
                language,
                &vocab,
                cfg.transcription.no_verbatim,
            )
            .await
        }
        // Spelled out rather than `_ =>` so a new backend fails here
        // instead of quietly becoming ElevenLabs.
        other => Err(anyhow::anyhow!("'{other}' is not a realtime backend")),
    };
    let mut session = match session_result {
        Ok(s) => {
            log_status(status_log, LogLevel::Info, "WebSocket connected");
            s
        }
        Err(e) => {
            log_status(status_log, LogLevel::Error, format!("Connection failed: {}", e));
            show_notification("Beamer", &format!("Connection failed: {}", e));
            return Err(e);
        }
    };

    // Raw PCM stream — VAD is handled server-side by the transcription backend
    let pipeline = match AudioPipeline::new() {
        Ok(p) => p,
        Err(e) => {
            log_status(status_log, LogLevel::Error, "Microphone not available");
            show_notification("Beamer", "Microphone not available");
            return Err(e);
        }
    };
    let (_stream, mut audio_rx) = pipeline.start()?;
    let mut audio_drop_count: u64 = 0;

    rec_state.set(RecordingState::Recording);
    log_status(status_log, LogLevel::Info, "Recording started");
    // Guard resumes playback on drop, on every exit path below.
    let media_pause = if cfg.recording.pause_media {
        crate::media::pause_media_if_playing()
    } else {
        None
    };
    crate::sounds::play_start_sound();

    // Whether the backend ever really transcribed (see `ClosedStream`).
    let mut saw_live_event = false;
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
                        match try_send_reserving(&session.audio_tx, transcription::AUDIO_SENTINEL_RESERVE, bytes) {
                            SendOutcome::Sent => {}
                            SendOutcome::Full => warn_channel_full(&mut audio_drop_count, "Realtime audio_tx"),
                            // Reader task exited: normal teardown, not backpressure.
                            SendOutcome::Closed => {}
                        }
                    }
                    _ => {
                        log_status(status_log, LogLevel::Warn, "Audio channel closed");
                        stop_reason = StopReason::AudioLost;
                        break;
                    }
                }
            }

            event = session.transcript_rx.recv() => {
                // ⚠️ The `None` arm must break: `recv()` on a closed channel
                // returns `None` forever, so falling through spins this loop
                // hot and starves the Dioxus scheduler — a frozen app.
                let Some(ev) = event else {
                    let closed = ClosedStream::classify(saw_live_event);
                    let message = closed.message(display_name);
                    tracing::warn!("{}", message);
                    log_status(status_log, closed.level(), message.clone());
                    if closed == ClosedStream::NeverStarted {
                        // Otherwise a refused connection looks like an empty recording.
                        show_notification("Beamer", &message);
                    }
                    stop_reason = StopReason::TranscriptLost;
                    break;
                };
                saw_live_event |= proves_session_live(&ev.kind);
                match ev.kind {
                    TranscriptKind::Final => {
                        if !ev.text.trim().is_empty() {
                            tracing::info!("[final] {}", ev.text);
                            log_status(status_log, LogLevel::Info, format!("[final] {}", ev.text));
                            sink::deliver(&ev.text, capture_mode, &backends, &paste_shortcut,
                                    last_injection, history, status_log, notes, tasks, config,
                                    note_passes).await;
                        }
                    }
                    TranscriptKind::Partial => {
                        if !ev.text.is_empty() {
                            tracing::debug!("[partial] {}", ev.text);
                        }
                    }
                    TranscriptKind::SessionStarted(ref sid) => {
                        tracing::info!("[session] started: {}", sid);
                        log_status(status_log, LogLevel::Info, format!("Session started: {}", sid));
                    }
                    TranscriptKind::Error(ref msg) => {
                        tracing::error!("[error] {}", msg);
                        log_status(status_log, LogLevel::Error, format!("Transcription error: {}", msg));
                    }
                    TranscriptKind::Info(ref msg) => {
                        tracing::info!("[info] {}", msg);
                        log_status(status_log, LogLevel::Info, msg.clone());
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
        stream_tail_audio(&mut audio_rx, &session.audio_tx, &mut audio_drop_count).await;
    }

    send_commit_sentinel(&session.audio_tx, status_log);
    log_status(status_log, LogLevel::Info, "Sent commit, waiting for final transcript...");
    drain_final_transcripts(
        &mut session, &backends, &paste_shortcut, last_injection, history, status_log,
        notes, tasks, config, capture_mode, note_passes,
    )
    .await;

    log_status(status_log, LogLevel::Info, "Recording stopped");
    Ok(())
}

/// Inject any remaining final transcripts, up to `FINAL_TRANSCRIPT_TIMEOUT_MS`
/// or until the backend closes the stream.
async fn drain_final_transcripts(
    session: &mut transcription::RealtimeSession,
    backends: &[String],
    paste_shortcut: &str,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    status_log: &mut Signal<StatusLog>,
    notes: &mut Signal<NoteStore>,
    tasks: &mut Signal<TaskStore>,
    config: &Signal<Config>,
    capture_mode: CaptureMode,
    note_passes: Coroutine<PipelineRequest>,
) {
    let deadline = tokio::time::Instant::now()
        + tokio::time::Duration::from_millis(FINAL_TRANSCRIPT_TIMEOUT_MS);
    loop {
        tokio::select! {
            event = session.transcript_rx.recv() => {
                match event {
                    Some(ev) => {
                        if let TranscriptKind::Final = ev.kind {
                            if !ev.text.trim().is_empty() {
                                tracing::info!("[final] {}", ev.text);
                                log_status(status_log, LogLevel::Info, format!("[final] {}", ev.text));
                                sink::deliver(&ev.text, capture_mode, backends, paste_shortcut,
                                        last_injection, history, status_log, notes, tasks, config,
                                        note_passes).await;
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
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
    log_status(status_log, LogLevel::Info, "Recording started (batch mode)");
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

    // Skip tail capture when the audio channel died (see the realtime path).
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
        format!("Sending {:.1}s of audio to {} batch API...", audio_secs, backend_label),
    );

    let start = tokio::time::Instant::now();
    let vocab = crate::config::vocabulary::Vocabulary::load()?.list().to_vec();
    let result = if backend == "voxtral_batch" {
        transcription::transcribe_voxtral_batch(api_key, pcm_buffer, &vocab).await
    } else {
        transcription::transcribe_batch(
            api_key,
            pcm_buffer,
            language,
            &vocab,
            cfg.transcription.no_verbatim,
        )
        .await
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

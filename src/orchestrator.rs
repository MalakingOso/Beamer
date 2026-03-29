use anyhow::Result;
use dioxus::prelude::*;
use futures_util::StreamExt;

use crate::audio::AudioPipeline;
use crate::config::Config;
use crate::hotkey::HotkeyEvent;
use crate::injection;
use crate::transcription::{self, TranscriptKind};
use crate::ui::history::TranscriptionHistory;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

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
    mut overlay_text: Signal<String>,
    mut last_injection: Signal<String>,
    mut history: Signal<TranscriptionHistory>,
    mut status_log: Signal<StatusLog>,
) {
    tracing::info!("Orchestrator started, waiting for hotkey events");
    log_status(&mut status_log, LogLevel::Info, "Orchestrator ready");

    while let Some(event) = hotkey_rx.next().await {
        match event {
            HotkeyEvent::RecordStart => {
                if let Err(e) = handle_recording(
                    &config,
                    &mut rec_state,
                    &mut overlay_text,
                    &mut last_injection,
                    &mut history,
                    &mut hotkey_rx,
                    &mut status_log,
                )
                .await
                {
                    tracing::error!("Recording session error: {}", e);
                    log_status(&mut status_log, LogLevel::Error, format!("Recording error: {}", e));
                    show_notification("Beamer", &format!("Recording error: {}", e));
                }
                rec_state.set(RecordingState::Idle);
                overlay_text.set(String::new());
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
    overlay_text: &mut Signal<String>,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    hotkey_rx: &mut UnboundedReceiver<HotkeyEvent>,
    status_log: &mut Signal<StatusLog>,
) -> Result<()> {
    let cfg = config.read().clone();
    let backend = &cfg.transcription.backend;
    let language = &cfg.transcription.language;
    let preferred_method = cfg.injection.preferred_method.clone();

    let (key_name, display_name) = match backend.as_str() {
        "voxtral" | "voxtral_batch" => ("mistral_api_key", "Voxtral"),
        _ => ("elevenlabs_api_key", "ElevenLabs"),
    };
    let api_key = crate::config::load_api_key(key_name);
    if api_key.is_empty() {
        log_status(status_log, LogLevel::Error, format!("No {} API key configured — open Settings", display_name));
        show_notification("Beamer", &format!("No {} API key configured. Open Settings to add one.", display_name));
        return Ok(());
    }

    if backend == "elevenlabs_batch" || backend == "voxtral_batch" {
        return handle_batch_recording(
            backend, &api_key, language, &preferred_method, &cfg,
            rec_state, overlay_text, last_injection, history, hotkey_rx, status_log,
        ).await;
    }

    log_status(status_log, LogLevel::Info, format!("Connecting to {} realtime...", display_name));
    let session_result = match backend.as_str() {
        "voxtral" => transcription::start_voxtral_session(&api_key).await,
        _ => transcription::start_elevenlabs_session(&api_key, language).await,
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

    rec_state.set(RecordingState::Recording);
    overlay_text.set("Listening...".to_string());
    log_status(status_log, LogLevel::Info, "Recording started");
    let did_pause = if cfg.recording.pause_media {
        crate::media::pause_media_if_playing()
    } else {
        false
    };
    crate::sounds::play_start_sound();

    loop {
        tokio::select! {
            hotkey_event = hotkey_rx.next() => {
                match hotkey_event {
                    Some(HotkeyEvent::RecordStop) | None => {
                        crate::sounds::play_stop_sound();
                        rec_state.set(RecordingState::Processing);
                        if did_pause {
                            crate::media::resume_media();
                        }

                        // Continue capturing audio briefly so the last word isn't clipped
                        let tail = tokio::time::Instant::now()
                            + tokio::time::Duration::from_millis(400);
                        loop {
                            tokio::select! {
                                chunk = audio_rx.recv() => {
                                    if let Some(bytes) = chunk {
                                        if !bytes.is_empty() {
                                            let _ = session.audio_tx.send(bytes);
                                        }
                                    }
                                }
                                _ = tokio::time::sleep_until(tail) => break,
                            }
                        }

                        // Empty Vec signals the backend to commit/finalize
                        let _ = session.audio_tx.send(Vec::new());
                        log_status(status_log, LogLevel::Info, "Sent commit, waiting for final transcript...");

                        // Drain any remaining final transcripts before closing
                        let deadline = tokio::time::Instant::now()
                            + tokio::time::Duration::from_millis(2000);
                        loop {
                            tokio::select! {
                                event = session.transcript_rx.recv() => {
                                    match event {
                                        Some(ev) => {
                                            if let TranscriptKind::Final = ev.kind {
                                                if !ev.text.trim().is_empty() {
                                                    tracing::info!("[final] {}", ev.text);
                                                    log_status(status_log, LogLevel::Info, format!("[final] {}", ev.text));
                                                    do_injection(&ev.text, &preferred_method, last_injection, history, overlay_text, status_log).await;
                                                }
                                            }
                                        }
                                        None => break,
                                    }
                                }
                                _ = tokio::time::sleep_until(deadline) => break,
                            }
                        }

                        log_status(status_log, LogLevel::Info, "Recording stopped");
                        break;
                    }
                    Some(HotkeyEvent::RecordStart) => {}
                }
            }

            chunk = audio_rx.recv() => {
                match chunk {
                    Some(bytes) if !bytes.is_empty() => {
                        let _ = session.audio_tx.send(bytes);
                    }
                    _ => {
                        log_status(status_log, LogLevel::Warn, "Audio channel closed");
                        break;
                    }
                }
            }

            event = session.transcript_rx.recv() => {
                if let Some(ev) = event {
                    match ev.kind {
                        TranscriptKind::Final => {
                            if !ev.text.trim().is_empty() {
                                tracing::info!("[final] {}", ev.text);
                                log_status(status_log, LogLevel::Info, format!("[final] {}", ev.text));
                                do_injection(&ev.text, &preferred_method, last_injection, history, overlay_text, status_log).await;
                            }
                        }
                        TranscriptKind::Partial => {
                            if !ev.text.is_empty() {
                                tracing::debug!("[partial] {}", ev.text);
                                overlay_text.set(ev.text);
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
    }

    Ok(())
}

/// Drive one batch recording session: capture mic → buffer all PCM → POST to ElevenLabs batch API.
async fn handle_batch_recording(
    backend: &str,
    api_key: &str,
    language: &str,
    preferred_method: &str,
    cfg: &Config,
    rec_state: &mut Signal<RecordingState>,
    overlay_text: &mut Signal<String>,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    hotkey_rx: &mut UnboundedReceiver<HotkeyEvent>,
    status_log: &mut Signal<StatusLog>,
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
    overlay_text.set("Listening...".to_string());
    log_status(status_log, LogLevel::Info, "Recording started (batch mode)");
    let did_pause = if cfg.recording.pause_media {
        crate::media::pause_media_if_playing()
    } else {
        false
    };
    crate::sounds::play_start_sound();

    // Collect all PCM audio into a buffer
    let mut pcm_buffer: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            hotkey_event = hotkey_rx.next() => {
                match hotkey_event {
                    Some(HotkeyEvent::RecordStop) | None => {
                        crate::sounds::play_stop_sound();
                        rec_state.set(RecordingState::Processing);
                        if did_pause {
                            crate::media::resume_media();
                        }

                        // Capture 400ms tail audio so the last word isn't clipped
                        let tail = tokio::time::Instant::now()
                            + tokio::time::Duration::from_millis(400);
                        loop {
                            tokio::select! {
                                chunk = audio_rx.recv() => {
                                    if let Some(bytes) = chunk {
                                        pcm_buffer.extend_from_slice(&bytes);
                                    }
                                }
                                _ = tokio::time::sleep_until(tail) => break,
                            }
                        }
                        break;
                    }
                    Some(HotkeyEvent::RecordStart) => {}
                }
            }
            chunk = audio_rx.recv() => {
                match chunk {
                    Some(bytes) if !bytes.is_empty() => {
                        pcm_buffer.extend_from_slice(&bytes);
                    }
                    _ => {
                        log_status(status_log, LogLevel::Warn, "Audio channel closed");
                        break;
                    }
                }
            }
        }
    }

    if pcm_buffer.is_empty() {
        log_status(status_log, LogLevel::Info, "No audio captured");
        return Ok(());
    }

    overlay_text.set("Transcribing...".to_string());
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
        transcription::transcribe_batch(api_key, pcm_buffer, language, &vocab).await
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
                do_injection(&text, preferred_method, last_injection, history, overlay_text, status_log).await;
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

/// Inject transcribed text into the focused window using the configured fallback chain.
async fn do_injection(
    text: &str,
    preferred_method: &str,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    overlay_text: &mut Signal<String>,
    status_log: &mut Signal<StatusLog>,
) {
    overlay_text.set(text.to_string());

    match injection::inject_text(text, preferred_method).await {
        Ok(result) => {
            let status = format!("{}: {}", result.method, result.target_info);
            tracing::info!("Injected via {}", status);
            log_status(status_log, LogLevel::Info, format!("Injected via {}", status));
            last_injection.set(status);
        }
        Err(e) => {
            tracing::error!("Injection failed: {}", e);
            log_status(status_log, LogLevel::Error, format!("Injection failed: {}", e));
            last_injection.set(format!("Failed: {}", e));
        }
    }

    history.write().append(text.to_string());
}

/// Show a Windows toast notification via WinRT (powershell app ID).
fn show_notification(title: &str, message: &str) {
    tracing::info!("Notification: {} - {}", title, message);
    if let Err(e) = winrt_notification::Toast::new(winrt_notification::Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(message)
        .show()
    {
        tracing::warn!("Failed to show notification: {}", e);
    }
}

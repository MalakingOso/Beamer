use anyhow::Result;
use dioxus::prelude::*;
use futures_util::StreamExt;

use crate::audio::{AudioEvent, AudioPipeline};
use crate::config::Config;
use crate::hotkey::HotkeyEvent;
use crate::injection;
use crate::transcription::{self, RealtimeSession, TranscriptEvent, TranscriptKind};
use crate::ui::history::TranscriptionHistory;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Central orchestration loop. Spawned as a Dioxus coroutine from App.
pub async fn run(
    mut hotkey_rx: UnboundedReceiver<HotkeyEvent>,
    config: Signal<Config>,
    mut is_recording: Signal<bool>,
    mut overlay_text: Signal<String>,
    mut last_injection: Signal<String>,
    mut history: Signal<TranscriptionHistory>,
    mut status_log: Signal<StatusLog>,
) {
    tracing::info!("Orchestrator started, waiting for hotkey events");
    log_status(&mut status_log, LogLevel::Info, "Orchestrator ready");

    // Main loop: wait for RecordStart events
    while let Some(event) = hotkey_rx.next().await {
        match event {
            HotkeyEvent::RecordStart => {
                if let Err(e) = handle_recording(
                    &config,
                    &mut is_recording,
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
                // Ensure clean state after any recording session
                is_recording.set(false);
                overlay_text.set(String::new());
            }
            HotkeyEvent::RecordStop => {
                // Ignore stop events when not recording
            }
        }
    }
}

/// Handle a single recording session from start to stop.
async fn handle_recording(
    config: &Signal<Config>,
    is_recording: &mut Signal<bool>,
    overlay_text: &mut Signal<String>,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    hotkey_rx: &mut UnboundedReceiver<HotkeyEvent>,
    status_log: &mut Signal<StatusLog>,
) -> Result<()> {
    let cfg = config.read().clone();
    let backend_name = &cfg.transcription.backend;
    let language = &cfg.transcription.language;

    // Load API key from keyring
    let api_key = load_api_key("elevenlabs_api_key");

    if api_key.is_empty() {
        log_status(status_log, LogLevel::Error, "No API key configured — open Settings");
        show_notification(
            "Beamer",
            "No API key configured. Open Settings to add one.",
        );
        return Ok(());
    }

    // Create audio pipeline
    let pipeline = match AudioPipeline::new() {
        Ok(p) => p,
        Err(e) => {
            log_status(status_log, LogLevel::Error, "Microphone not available");
            show_notification("Beamer", "Microphone not available");
            return Err(e);
        }
    };

    let (_stream, mut audio_rx) = pipeline.start(
        cfg.advanced.vad_aggressiveness,
        cfg.advanced.pre_buffer_ms,
        cfg.advanced.silence_timeout_ms,
    )?;
    // _stream must stay alive — dropping it stops audio capture

    // Create transcription backend
    let backend = transcription::create_backend(backend_name, api_key);

    // Check if backend supports realtime
    let is_realtime = backend_name.contains("realtime");
    let mut realtime_session: Option<RealtimeSession> = None;

    if is_realtime {
        log_status(status_log, LogLevel::Info, "Connecting to ElevenLabs realtime...");
        match backend.start_realtime_session(language).await {
            Ok(Some(session)) => {
                log_status(status_log, LogLevel::Info, "WebSocket connected");
                realtime_session = Some(session);
            }
            Ok(None) => {
                log_status(status_log, LogLevel::Warn, "Backend doesn't support realtime, using batch");
            }
            Err(e) => {
                log_status(
                    status_log,
                    LogLevel::Error,
                    format!("Realtime connection failed: {}", e),
                );
                show_notification("Beamer", &format!("Realtime connection failed: {}", e));
                return Err(e);
            }
        }
    }

    // We're recording
    is_recording.set(true);
    overlay_text.set("Listening...".to_string());
    log_status(
        status_log,
        LogLevel::Info,
        format!("Recording started ({})", backend_name),
    );

    // Load vocabulary for batch transcription
    let vocab = crate::config::vocabulary::Vocabulary::load()
        .map(|v| v.list().to_vec())
        .unwrap_or_default();

    let preferred_method = cfg.injection.preferred_method.clone();
    let lang = language.to_string();

    // Inner recording loop
    loop {
        tokio::select! {
            // Hotkey stop event
            hotkey_event = hotkey_rx.next() => {
                match hotkey_event {
                    Some(HotkeyEvent::RecordStop) | None => {
                        // Signal EOS to realtime session
                        if let Some(ref session) = realtime_session {
                            let _ = session.audio_tx.send(Vec::new());
                            log_status(status_log, LogLevel::Info, "Sent commit, waiting for final transcript...");
                        }
                        // Drain any remaining transcript events
                        if let Some(ref mut session) = realtime_session {
                            drain_final_transcripts(
                                session,
                                &preferred_method,
                                last_injection,
                                history,
                                overlay_text,
                                status_log,
                            ).await;
                        }
                        log_status(status_log, LogLevel::Info, "Recording stopped");
                        break;
                    }
                    Some(HotkeyEvent::RecordStart) => {
                        // Ignore spurious start while already recording
                    }
                }
            }

            // Audio events from VAD pipeline
            audio_event = audio_rx.recv() => {
                match audio_event {
                    Some(AudioEvent::SpeechStart) => {
                        overlay_text.set("Listening...".to_string());
                    }
                    Some(AudioEvent::AudioChunk(chunk)) => {
                        // Forward chunks to realtime session
                        if let Some(ref session) = realtime_session {
                            let _ = session.audio_tx.send(chunk);
                        }
                    }
                    Some(AudioEvent::AudioReady(wav)) => {
                        // Batch mode: transcribe complete utterance
                        if realtime_session.is_none() {
                            overlay_text.set("Processing...".to_string());
                            log_status(status_log, LogLevel::Info, "Transcribing audio (batch)...");
                            match backend.transcribe_batch(wav, &lang, &vocab).await {
                                Ok(text) if !text.trim().is_empty() => {
                                    log_status(status_log, LogLevel::Info, format!("[final] {}", text));
                                    do_injection(
                                        &text,
                                        &preferred_method,
                                        last_injection,
                                        history,
                                        overlay_text,
                                        status_log,
                                    ).await;
                                }
                                Ok(_) => {
                                    overlay_text.set("(no speech detected)".to_string());
                                    log_status(status_log, LogLevel::Info, "No speech detected");
                                }
                                Err(e) => {
                                    tracing::error!("Transcription failed: {}", e);
                                    log_status(status_log, LogLevel::Error, format!("Transcription failed: {}", e));
                                    show_notification("Beamer", &format!("Transcription failed: {}", e));
                                    overlay_text.set("Error".to_string());
                                }
                            }
                        }
                    }
                    Some(AudioEvent::SpeechEnd) => {
                        // Batch: already handled in AudioReady
                        // Realtime: transcript events arrive via transcript_rx
                    }
                    None => {
                        // Audio channel closed
                        log_status(status_log, LogLevel::Warn, "Audio channel closed");
                        break;
                    }
                }
            }

            // Realtime transcript events
            transcript_event = recv_transcript(&mut realtime_session) => {
                if let Some(event) = transcript_event {
                    match event.kind {
                        TranscriptKind::Final => {
                            if !event.text.trim().is_empty() {
                                log_status(status_log, LogLevel::Info, format!("[final] {}", event.text));
                                do_injection(
                                    &event.text,
                                    &preferred_method,
                                    last_injection,
                                    history,
                                    overlay_text,
                                    status_log,
                                ).await;
                            }
                        }
                        TranscriptKind::Partial => {
                            if !event.text.is_empty() {
                                overlay_text.set(event.text);
                            }
                        }
                        TranscriptKind::SessionStarted(ref sid) => {
                            log_status(status_log, LogLevel::Info, format!("Session started: {}", sid));
                        }
                        TranscriptKind::Error(ref msg) => {
                            log_status(status_log, LogLevel::Error, format!("ElevenLabs: {}", msg));
                        }
                        TranscriptKind::Info(ref msg) => {
                            log_status(status_log, LogLevel::Info, msg.clone());
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Inject transcribed text into the focused window and update state.
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

/// Drain any remaining final transcript events after signaling EOS.
async fn drain_final_transcripts(
    session: &mut RealtimeSession,
    preferred_method: &str,
    last_injection: &mut Signal<String>,
    history: &mut Signal<TranscriptionHistory>,
    overlay_text: &mut Signal<String>,
    status_log: &mut Signal<StatusLog>,
) {
    // Give the server a moment to send final transcripts
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(500);
    loop {
        tokio::select! {
            event = session.transcript_rx.recv() => {
                match event {
                    Some(ev) => {
                        match ev.kind {
                            TranscriptKind::Final if !ev.text.trim().is_empty() => {
                                log_status(status_log, LogLevel::Info, format!("[final] {}", ev.text));
                                do_injection(&ev.text, preferred_method, last_injection, history, overlay_text, status_log).await;
                            }
                            _ => {}
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                break;
            }
        }
    }
}

/// Async helper for select!: receives from realtime session or pends forever if none.
async fn recv_transcript(session: &mut Option<RealtimeSession>) -> Option<TranscriptEvent> {
    match session {
        Some(ref mut s) => s.transcript_rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Load an API key from Windows Credential Manager.
fn load_api_key(name: &str) -> String {
    keyring::Entry::new("beamer", name)
        .and_then(|e| e.get_password())
        .unwrap_or_default()
}

/// Show a Windows tray balloon notification.
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

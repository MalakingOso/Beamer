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
                is_recording.set(false);
                overlay_text.set(String::new());
            }
            HotkeyEvent::RecordStop => {}
        }
    }
}

/// Handle a single recording session, mirroring ws_test.rs flow:
/// connect WS → start mic → stream all audio → on stop: commit + drain finals
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
    let backend = &cfg.transcription.backend;
    let language = &cfg.transcription.language;
    let preferred_method = cfg.injection.preferred_method.clone();

    // Load API key for selected backend
    let (key_name, display_name) = match backend.as_str() {
        "voxtral" => ("mistral_api_key", "Voxtral"),
        _ => ("elevenlabs_api_key", "ElevenLabs"),
    };
    let api_key = load_api_key(key_name);
    if api_key.is_empty() {
        log_status(status_log, LogLevel::Error, format!("No {} API key configured — open Settings", display_name));
        show_notification("Beamer", &format!("No {} API key configured. Open Settings to add one.", display_name));
        return Ok(());
    }

    // Connect WebSocket first
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

    // Start mic capture (raw PCM, no VAD — like ws_test.rs)
    let pipeline = match AudioPipeline::new() {
        Ok(p) => p,
        Err(e) => {
            log_status(status_log, LogLevel::Error, "Microphone not available");
            show_notification("Beamer", "Microphone not available");
            return Err(e);
        }
    };
    let (_stream, mut audio_rx) = pipeline.start()?;

    is_recording.set(true);
    overlay_text.set("Listening...".to_string());
    log_status(status_log, LogLevel::Info, "Recording started");

    // Main loop: forward audio + receive transcripts (mirrors ws_test.rs select! loop)
    loop {
        tokio::select! {
            // Stop event
            hotkey_event = hotkey_rx.next() => {
                match hotkey_event {
                    Some(HotkeyEvent::RecordStop) | None => {
                        // Send commit (like ws_test.rs Ctrl+C handler)
                        let _ = session.audio_tx.send(Vec::new());
                        log_status(status_log, LogLevel::Info, "Sent commit, waiting for final transcript...");

                        // Wait for final transcripts (ws_test.rs uses 1000ms)
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

            // Forward mic audio to WebSocket
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

            // Receive transcript events
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

/// Inject transcribed text into the focused window.
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

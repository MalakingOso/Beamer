//! Start/stop beeps for a dictation, played by the orchestrator. Playback runs
//! on a dedicated output thread, so callers never wait on the audio device.

use std::io::Cursor;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

// Embedded so the binary needs no runtime asset files.
const START_SOUND: &[u8] = include_bytes!("../assets/startsound.mp3");
const END_SOUND: &[u8] = include_bytes!("../assets/endsound.mp3");

/// Play the recording-start sound. Non-blocking; queued on the audio thread.
pub fn play_start_sound() {
    tracing::info!("play_start_sound called");
    play(START_SOUND);
}

/// Play the recording-stop sound. Non-blocking; queued on the audio thread.
pub fn play_stop_sound() {
    tracing::info!("play_stop_sound called");
    play(END_SOUND);
}

/// Pre-start the audio thread during warmup; on Windows this also opens the
/// output device, so the first beep pays no WASAPI activation cost.
/// Idempotent; `play` starts the thread lazily anyway.
pub fn warm() {
    channel();
}

/// Queue `data` for playback, starting the audio thread on first use.
fn play(data: &'static [u8]) {
    if channel().send(data).is_err() {
        // The thread only exits when the output device failed to open, which
        // stays failed for the process. Sound is best-effort: log and move on.
        tracing::warn!("audio output thread is gone; sound dropped");
    }
}

fn channel() -> &'static Sender<&'static [u8]> {
    static CHANNEL: OnceLock<Sender<&'static [u8]>> = OnceLock::new();
    CHANNEL.get_or_init(spawn_audio_thread)
}

/// Spawn the thread that owns the `rodio` output and plays beeps as they arrive.
///
/// Windows: WASAPI activation is expensive, so one `OutputStream` is held for
/// the process lifetime (it is not `Send`, so it lives on this thread).
/// Elsewhere: a stream is opened per beep and dropped after it, because an idle
/// open stream caused audible hiss on some PipeWire/ALSA setups.
fn spawn_audio_thread() -> Sender<&'static [u8]> {
    let (tx, rx) = mpsc::channel::<&'static [u8]>();
    let _ = std::thread::Builder::new()
        .name("beamer-sounds".into())
        .spawn(move || {
            // WASAPI needs COM; this thread uses its own MTA apartment.
            #[cfg(target_os = "windows")]
            let com_initialized = unsafe {
                windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                )
                .is_ok()
            };

            // On Windows keep a persistent stream for fast activation, but allow
            // re-opening it if startup failed or a device/sleep change invalidates it.
            #[cfg(target_os = "windows")]
            let mut persistent: Option<(rodio::OutputStream, rodio::OutputStreamHandle)> =
                match rodio::OutputStream::try_default() {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::warn!("Failed to open audio output device on startup: {}", e);
                        None
                    }
                };

            for data in rx {
                #[cfg(not(target_os = "windows"))]
                let stream = match rodio::OutputStream::try_default() {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("Failed to open audio output device: {}", e);
                        continue;
                    }
                };
                #[cfg(not(target_os = "windows"))]
                let (_stream, handle) = &stream;

                #[cfg(target_os = "windows")]
                let sink = {
                    if persistent.is_none() {
                        match rodio::OutputStream::try_default() {
                            Ok(v) => persistent = Some(v),
                            Err(e) => {
                                tracing::warn!("Failed to open audio output device: {}", e);
                                continue;
                            }
                        }
                    }
                    let first = persistent
                        .as_ref()
                        .map(|(_, h)| rodio::Sink::try_new(h));
                    match first {
                        Some(Ok(v)) => v,
                        Some(Err(e)) => {
                            // Device switch / sleep-wake invalidated the stream; reopen once.
                            tracing::warn!("Audio sink failed ({}); reopening output device", e);
                            persistent = None;
                            match rodio::OutputStream::try_default() {
                                Ok(v) => {
                                    persistent = Some(v);
                                    match persistent.as_ref().map(|(_, h)| rodio::Sink::try_new(h)) {
                                        Some(Ok(s)) => s,
                                        Some(Err(e2)) => {
                                            tracing::warn!("Failed to create audio sink after reopen: {}", e2);
                                            persistent = None;
                                            continue;
                                        }
                                        None => continue,
                                    }
                                }
                                Err(e2) => {
                                    tracing::warn!("Failed to reopen audio output device: {}", e2);
                                    continue;
                                }
                            }
                        }
                        None => continue,
                    }
                };

                #[cfg(not(target_os = "windows"))]
                let sink = match rodio::Sink::try_new(handle) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("Failed to create audio sink: {}", e);
                        continue;
                    }
                };

                let source = match rodio::Decoder::new(Cursor::new(data)) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("Failed to decode sound effect: {}", e);
                        continue;
                    }
                };
                sink.append(source);
                #[cfg(target_os = "windows")]
                sink.detach();
                #[cfg(not(target_os = "windows"))]
                sink.sleep_until_end();
                tracing::debug!("Sound queued for playback ({} bytes)", data.len());
            }

            #[cfg(target_os = "windows")]
            {
                drop(persistent);
                if com_initialized {
                    unsafe { windows::Win32::System::Com::CoUninitialize() };
                }
            }
        });
    tx
}

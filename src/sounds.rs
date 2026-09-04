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

/// Pre-start the audio thread during warmup so first playback pays no device
/// activation cost. Idempotent; `play` also starts it lazily.
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

/// One `rodio::OutputStream` for the process, owned by a dedicated thread.
///
/// Opening the stream (WASAPI activation on Windows) is expensive, so it is
/// paid once here instead of per beep. `OutputStream` is not `Send` and must
/// live on the thread that created it.
fn spawn_audio_thread() -> Sender<&'static [u8]> {
    let (tx, rx) = mpsc::channel::<&'static [u8]>();
    std::thread::spawn(move || {
        // WASAPI needs COM; this thread uses its own MTA apartment.
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }

        let (_stream, handle) = match rodio::OutputStream::try_default() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to open audio output device: {}", e);
                return;
            }
        };

        // `_stream` must outlive the loop: dropping it tears down all sinks.
        for data in rx {
            let sink = match rodio::Sink::try_new(&handle) {
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
            // Detached so playback completes without blocking this loop.
            sink.detach();
            tracing::debug!("Sound queued for playback ({} bytes)", data.len());
        }
    });
    tx
}

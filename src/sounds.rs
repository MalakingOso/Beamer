use std::io::Cursor;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

// Embedded at compile time so the binary is self-contained (no runtime asset loading)
const START_SOUND: &[u8] = include_bytes!("../assets/startsound.mp3");
const END_SOUND: &[u8] = include_bytes!("../assets/endsound.mp3");

/// Play the recording-start sound. Queued on the shared audio thread; returns
/// immediately.
pub fn play_start_sound() {
    tracing::info!("play_start_sound called");
    play(START_SOUND);
}

/// Play the recording-stop sound. Queued on the shared audio thread; returns
/// immediately.
pub fn play_stop_sound() {
    tracing::info!("play_stop_sound called");
    play(END_SOUND);
}

/// Start the shared audio thread ahead of the first sound, so the one-time
/// device activation lands during the warmup splash instead of on the user's
/// first hotkey press. Called from `warmup::warm_all`. Safe to call more than
/// once, and safe to never call at all (`play` starts the thread itself on
/// first use; this only moves *when* that happens).
pub fn warm() {
    channel();
}

/// Queue `data` for playback on the process's one audio-output thread,
/// starting that thread on first use.
fn play(data: &'static [u8]) {
    if channel().send(data).is_err() {
        // The thread exits only if opening the output device failed (see
        // `spawn_audio_thread`); once that has happened once, it stays
        // failed for the rest of the process, same as an unplugged output
        // device would. Audio feedback must never disrupt or delay
        // dictation, so this is a log line, not a retry or a panic.
        tracing::warn!("audio output thread is gone; sound dropped");
    }
}

/// The channel to the process's one audio-output thread, spawning it on
/// first use.
fn channel() -> &'static Sender<&'static [u8]> {
    static CHANNEL: OnceLock<Sender<&'static [u8]>> = OnceLock::new();
    CHANNEL.get_or_init(spawn_audio_thread)
}

/// One `rodio::OutputStream` for the whole process, owned by a dedicated
/// thread, fed sounds to play over `rx`.
///
/// On Windows, opening an `OutputStream` is a full WASAPI activation
/// (`IMMDeviceEnumerator`, `IAudioClient::Initialize`, format negotiation).
/// The old code paid that cost on every single beep, on a fresh thread each
/// time. Here it is paid once: this thread opens the stream and keeps it
/// alive for the process, and each sound gets only a fresh `Sink` against the
/// same stream, which is cheap.
///
/// `rodio::OutputStream` is not `Send`. On Windows it wraps a WASAPI client
/// that is apartment-bound, so it cannot be built on one thread and handed
/// to another. It has to be created, and kept alive, on the thread that owns
/// it, which is this one.
fn spawn_audio_thread() -> Sender<&'static [u8]> {
    let (tx, rx) = mpsc::channel::<&'static [u8]>();
    std::thread::spawn(move || {
        // WASAPI requires COM. Dioxus already initialized it as STA on
        // main, so this thread needs its own MTA apartment. Preserved from
        // the old per-beep thread; it now runs once instead of once per beep.
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

        // `_stream` lives for the rest of this closure, i.e. for the process.
        // Dropping it would tear down the output and take every `Sink` built
        // on `handle` down with it, which is exactly why the old code slept
        // 50ms after each sound before its own stream dropped. With one
        // stream alive for good, there is nothing left to sleep for.
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
            // Hands the sink off to rodio's own bookkeeping so it plays to
            // completion and cleans itself up; this loop moves straight on to
            // the next queued sound rather than blocking the thread until
            // playback ends.
            sink.detach();
            tracing::debug!("Sound queued for playback ({} bytes)", data.len());
        }
    });
    tx
}

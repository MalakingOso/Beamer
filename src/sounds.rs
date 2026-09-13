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

/// Pre-start the audio thread during warmup. On Windows this also opens the
/// output device up front, so first playback pays no WASAPI activation
/// cost; elsewhere device activation happens lazily on first `play`, off
/// the caller's thread either way. Idempotent; `play` also starts it
/// lazily.
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

/// Dedicated thread that owns the `rodio` output device and plays beeps as
/// they arrive on the channel.
///
/// On Windows, WASAPI device activation is expensive, so one
/// `rodio::OutputStream` is opened here and held for the process lifetime,
/// paying that cost once instead of per beep. `OutputStream` is not `Send`
/// and must live on the thread that created it.
///
/// On other platforms, device activation is cheap, and holding a `cpal`
/// output stream open while idle (no `Sink` ever attached, for hours at a
/// time) produced audible intermittent hiss on some PipeWire/ALSA setups —
/// an idle stream still occupies the device, and it can glitch on
/// underrun. So there, the stream is opened fresh per beep and dropped as
/// soon as playback finishes.
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

        // `_stream` must outlive playback: dropping it tears down all sinks.
        #[cfg(target_os = "windows")]
        let (_stream, persistent_handle) = match rodio::OutputStream::try_default() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to open audio output device: {}", e);
                return;
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
            let handle = &persistent_handle;

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
            // Windows: detach so playback completes without blocking this
            // loop — the persistent stream keeps flowing regardless.
            #[cfg(target_os = "windows")]
            sink.detach();
            // Elsewhere: block until the beep finishes, so the freshly
            // opened `stream` above stays alive for its whole duration and
            // then closes the device again before the next message.
            #[cfg(not(target_os = "windows"))]
            sink.sleep_until_end();
            tracing::debug!("Sound queued for playback ({} bytes)", data.len());
        }
    });
    tx
}

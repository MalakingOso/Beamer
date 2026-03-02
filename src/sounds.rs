use std::io::Cursor;

// Embedded at compile time so the binary is self-contained (no runtime asset loading)
const START_SOUND: &[u8] = include_bytes!("../assets/startsound.mp3");
const END_SOUND: &[u8] = include_bytes!("../assets/endsound.mp3");

/// Play the recording-start sound. Fire-and-forget on a background thread.
pub fn play_start_sound() {
    tracing::info!("play_start_sound called");
    play(START_SOUND);
}

/// Play the recording-stop sound. Fire-and-forget on a background thread.
pub fn play_stop_sound() {
    tracing::info!("play_stop_sound called");
    play(END_SOUND);
}

/// Decode and play an mp3 buffer on a new OS thread.
/// Uses a dedicated thread because rodio's WASAPI backend requires per-thread
/// COM initialization, and the Dioxus WebView2 process already owns COM on main.
fn play(data: &'static [u8]) {
    std::thread::spawn(move || {
        // WASAPI requires COM — Dioxus already initialized it as STA on main,
        // so this thread needs its own MTA apartment

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
        let sink = match rodio::Sink::try_new(&handle) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to create audio sink: {}", e);
                return;
            }
        };
        let cursor = Cursor::new(data);
        let source = match rodio::Decoder::new(cursor) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to decode sound effect: {}", e);
                return;
            }
        };
        sink.append(source);
        sink.sleep_until_end();
        // Without this delay, dropping the OutputStream can cut off the last
        // few milliseconds on some WASAPI drivers
        std::thread::sleep(std::time::Duration::from_millis(50));
        tracing::debug!("Sound playback finished ({} bytes)", data.len());
    });
}

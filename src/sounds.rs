use std::io::Cursor;

const START_SOUND: &[u8] = include_bytes!("../assets/startsound.mp3");
const END_SOUND: &[u8] = include_bytes!("../assets/endsound.mp3");

/// Play the recording-start sound on a background thread (fire-and-forget).
pub fn play_start_sound() {
    play(START_SOUND);
}

/// Play the recording-stop sound on a background thread (fire-and-forget).
pub fn play_stop_sound() {
    play(END_SOUND);
}

fn play(data: &'static [u8]) {
    std::thread::spawn(move || {
        let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
            tracing::warn!("Failed to open audio output device for sound effect");
            return;
        };
        let Ok(sink) = rodio::Sink::try_new(&handle) else {
            tracing::warn!("Failed to create audio sink for sound effect");
            return;
        };
        let cursor = Cursor::new(data);
        let Ok(source) = rodio::Decoder::new(cursor) else {
            tracing::warn!("Failed to decode sound effect MP3");
            return;
        };
        sink.append(source);
        sink.sleep_until_end();
    });
}

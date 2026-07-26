/// Check if audio is currently being output on the default render device.
#[cfg(target_os = "windows")]
fn is_audio_playing() -> bool {
    is_audio_playing_wasapi()
}

/// Proof that Beamer paused playback, and the handle for undoing it.
///
/// Returned only when a pause actually happened, so "did we pause?" is carried
/// in the type rather than a bare `bool` the caller might forget to act on. On
/// Linux it remembers the D-Bus bus name of the *specific* player that was
/// paused: resuming used to just grab the first paused player it could find,
/// which meant that if you had (say) Spotify already paused and VLC playing,
/// Beamer paused VLC and then resumed Spotify.
///
/// `Drop` resumes, so every exit path out of a recording session restores
/// playback — including the error paths (mic lost mid-recording, transcription
/// failure) that previously left the user's music paused with no explanation.
/// `drop(guard)` at the point where playback should come back.
#[must_use = "dropping this immediately resumes playback; hold it for the duration of the recording"]
pub struct MediaPause {
    /// D-Bus bus name of the paused player. Absent on Windows, where the
    /// play/pause key is broadcast to the shell rather than addressed to one
    /// player, so there's no identity to remember.
    #[cfg(not(target_os = "windows"))]
    bus_name: String,
}

impl MediaPause {
    fn resume_inner(&mut self) {
        #[cfg(target_os = "windows")]
        {
            send_media_play_pause();
        }
        #[cfg(not(target_os = "windows"))]
        {
            match find_mpris_player_by_bus_name(&self.bus_name) {
                Some(player) => match player.play() {
                    Ok(()) => tracing::info!("Resumed MPRIS player: {}", self.bus_name),
                    Err(e) => tracing::warn!(
                        "Failed to resume MPRIS player {}: {}",
                        self.bus_name,
                        e
                    ),
                },
                // The player quit while we were recording. Nothing to resume,
                // and deliberately no fallback to "some other paused player" —
                // that's the bug this guard exists to prevent.
                None => tracing::info!(
                    "MPRIS player {} is gone — nothing to resume",
                    self.bus_name
                ),
            }
        }
    }
}

impl Drop for MediaPause {
    fn drop(&mut self) {
        self.resume_inner();
    }
}

/// Pause media only if audio is currently playing.
/// Returns a [`MediaPause`] guard when a pause actually happened.
pub fn pause_media_if_playing() -> Option<MediaPause> {
    #[cfg(target_os = "windows")]
    {
        if is_audio_playing() {
            send_media_play_pause();
            return Some(MediaPause {});
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        let player = find_playing_mpris_player()?;
        let bus_name = player.bus_name().to_string();
        match player.pause() {
            Ok(()) => {
                tracing::info!("Paused MPRIS player: {}", bus_name);
                Some(MediaPause { bus_name })
            }
            Err(e) => {
                tracing::warn!("Failed to pause MPRIS player {}: {}", bus_name, e);
                None
            }
        }
    }
}

/// Find the first MPRIS2 player that is currently Playing.
#[cfg(not(target_os = "windows"))]
fn find_playing_mpris_player() -> Option<mpris::Player> {
    let finder = mpris::PlayerFinder::new().ok()?;
    finder.find_all().ok()?.into_iter().find(|p| {
        p.get_playback_status().ok() == Some(mpris::PlaybackStatus::Playing)
    })
}

/// Find one specific MPRIS2 player by its D-Bus bus name.
///
/// Matched on bus name rather than `PlayerFinder::find_by_name`, which matches
/// on the human-facing `Identity` property and so can't distinguish two
/// instances of the same application.
#[cfg(not(target_os = "windows"))]
fn find_mpris_player_by_bus_name(bus_name: &str) -> Option<mpris::Player> {
    let finder = mpris::PlayerFinder::new().ok()?;
    finder
        .find_all()
        .ok()?
        .into_iter()
        .find(|p| p.bus_name() == bus_name)
}

#[cfg(target_os = "windows")]
fn is_audio_playing_wasapi() -> bool {
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(_) => return false,
            };

        let device = match enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) {
            Ok(d) => d,
            Err(_) => return false,
        };

        let meter: IAudioMeterInformation = match device.Activate(CLSCTX_ALL, None) {
            Ok(m) => m,
            Err(_) => return false,
        };

        match meter.GetPeakValue() {
            Ok(peak) => peak > 0.0001,
            Err(_) => false,
        }
    }
}

#[cfg(target_os = "windows")]
fn send_media_play_pause() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };

    const VK_MEDIA_PLAY_PAUSE: VIRTUAL_KEY = VIRTUAL_KEY(0xB3);

    let inputs = [
        make_key_input(VK_MEDIA_PLAY_PAUSE, false),
        make_key_input(VK_MEDIA_PLAY_PAUSE, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(target_os = "windows")]
fn make_key_input(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    key_up: bool,
) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    };

    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags = KEYEVENTF_KEYUP;
    }

    INPUT {
        r#type: INPUT_TYPE(1),
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

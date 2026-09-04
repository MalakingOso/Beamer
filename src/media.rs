/// Check if audio is currently being output on the default render device.
#[cfg(target_os = "windows")]
fn is_audio_playing() -> bool {
    is_audio_playing_wasapi()
}

/// Guard proving Beamer paused playback; `Drop` resumes it, so all exit paths
/// (including errors) restore playback. On Linux it remembers the specific
/// player's D-Bus bus name — resuming "any paused player" once resumed the
/// wrong one.
#[must_use = "dropping this immediately resumes playback; hold it for the duration of the recording"]
pub struct MediaPause {
    /// Absent on Windows, where play/pause is broadcast, not addressed.
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
                // Player quit mid-recording: nothing to resume, and no
                // fallback to another player (see `MediaPause`).
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

/// Pause media if playing; returns a [`MediaPause`] guard when it did.
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

/// First MPRIS2 player currently Playing.
#[cfg(not(target_os = "windows"))]
fn find_playing_mpris_player() -> Option<mpris::Player> {
    let finder = mpris::PlayerFinder::new().ok()?;
    finder.find_all().ok()?.into_iter().find(|p| {
        p.get_playback_status().ok() == Some(mpris::PlaybackStatus::Playing)
    })
}

/// MPRIS2 player by D-Bus bus name (`find_by_name` matches human-facing
/// `Identity`, which can't tell two instances of one app apart).
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
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, VIRTUAL_KEY};

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

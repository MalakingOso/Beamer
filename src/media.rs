/// Check if audio is currently being output on the default render device.
#[cfg(target_os = "windows")]
fn is_audio_playing() -> bool {
    is_audio_playing_wasapi()
}

/// Pause media only if audio is currently playing.
/// Returns `true` if we actually sent the pause keystroke.
pub fn pause_media_if_playing() -> bool {
    #[cfg(target_os = "windows")]
    if is_audio_playing() {
        send_media_play_pause();
        return true;
    }
    #[cfg(not(target_os = "windows"))]
    if let Some(player) = find_playing_mpris_player() {
        if player.pause().is_ok() {
            tracing::info!("Paused MPRIS player: {}", player.bus_name());
            return true;
        }
    }
    false
}

/// Resume media by sending the play/pause key.
/// Only call this when we know we previously paused.
pub fn resume_media() {
    #[cfg(target_os = "windows")]
    send_media_play_pause();
    #[cfg(not(target_os = "windows"))]
    {
        // Find a paused player and resume it
        if let Some(player) = find_paused_mpris_player() {
            if let Err(e) = player.play() {
                tracing::warn!("Failed to resume MPRIS player: {}", e);
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

/// Find the first MPRIS2 player that is currently Paused.
#[cfg(not(target_os = "windows"))]
fn find_paused_mpris_player() -> Option<mpris::Player> {
    let finder = mpris::PlayerFinder::new().ok()?;
    finder.find_all().ok()?.into_iter().find(|p| {
        p.get_playback_status().ok() == Some(mpris::PlaybackStatus::Paused)
    })
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

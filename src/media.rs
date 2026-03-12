use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};

const VK_MEDIA_PLAY_PAUSE: VIRTUAL_KEY = VIRTUAL_KEY(0xB3);

/// Check if audio is currently being output on the default render device.
/// Uses WASAPI IAudioMeterInformation to read the peak level.
pub fn is_audio_playing() -> bool {
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

/// Pause media only if audio is currently playing.
/// Returns `true` if we actually sent the pause keystroke.
pub fn pause_media_if_playing() -> bool {
    if is_audio_playing() {
        send_media_play_pause();
        true
    } else {
        false
    }
}

/// Resume media by sending the play/pause key.
/// Only call this when we know we previously paused.
pub fn resume_media() {
    send_media_play_pause();
}

fn send_media_play_pause() {
    let inputs = [
        make_key_input(VK_MEDIA_PLAY_PAUSE, false),
        make_key_input(VK_MEDIA_PLAY_PAUSE, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn make_key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    let mut flags = windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags = KEYEVENTF_KEYUP;
    }

    INPUT {
        r#type: INPUT_TYPE(1), // INPUT_KEYBOARD
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

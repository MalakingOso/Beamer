use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};

const VK_MEDIA_PLAY_PAUSE: VIRTUAL_KEY = VIRTUAL_KEY(0xB3);

/// Send a media play/pause keystroke to toggle whatever media player is active.
pub fn toggle_media_playback() {
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

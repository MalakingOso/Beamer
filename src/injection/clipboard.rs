use anyhow::Result;
use arboard::Clipboard;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};

/// Last-resort injection: sets clipboard text, sends Ctrl+V, then restores
/// the previous clipboard contents. Works universally but briefly clobbers
/// the user's clipboard and relies on timing heuristics.
pub fn inject_via_clipboard(text: &str) -> Result<bool> {
    let mut clipboard = Clipboard::new()?;

    let saved = clipboard.get_text().ok();
    clipboard.set_text(text)?;

    // Win32 clipboard updates are async — without a delay, SendInput can
    // paste stale content on slower machines
    std::thread::sleep(std::time::Duration::from_millis(50));

    send_ctrl_v();

    // Give the target app time to process the Ctrl+V before we overwrite
    // the clipboard with the restored content
    std::thread::sleep(std::time::Duration::from_millis(500));

    if let Some(saved_text) = saved {
        let _ = clipboard.set_text(saved_text);
    }

    Ok(true)
}

fn send_ctrl_v() {
    let vk_control = VIRTUAL_KEY(0x11); // VK_CONTROL
    let vk_v = VIRTUAL_KEY(0x56); // VK_V

    let inputs = [
        make_key_input(vk_control, false),
        make_key_input(vk_v, false),
        make_key_input(vk_v, true),
        make_key_input(vk_control, true),
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

use anyhow::Result;
use arboard::Clipboard;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};

/// Inject text via clipboard paste (Ctrl+V)
/// Saves and restores the previous clipboard contents
pub fn inject_via_clipboard(text: &str) -> Result<bool> {
    let mut clipboard = Clipboard::new()?;

    // Save current clipboard
    let saved = clipboard.get_text().ok();

    // Set our text
    clipboard.set_text(text)?;

    // Small delay to ensure clipboard is ready
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Simulate Ctrl+V
    send_ctrl_v();

    // Wait for paste to complete
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Restore clipboard
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

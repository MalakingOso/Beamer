use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_TYPE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY,
};

/// Inject text via Win32 SendInput with Unicode key events
pub fn inject_via_sendinput(text: &str) -> Result<bool> {
    let mut inputs: Vec<INPUT> = Vec::new();

    for ch in text.encode_utf16() {
        // Key down
        inputs.push(make_unicode_input(ch, false));
        // Key up
        inputs.push(make_unicode_input(ch, true));
    }

    if inputs.is_empty() {
        return Ok(true);
    }

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };

    if sent as usize != inputs.len() {
        tracing::warn!(
            "SendInput: sent {} of {} events",
            sent,
            inputs.len()
        );
    }

    Ok(sent > 0)
}

fn make_unicode_input(char_code: u16, key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }

    INPUT {
        r#type: INPUT_TYPE(1), // INPUT_KEYBOARD
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: char_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

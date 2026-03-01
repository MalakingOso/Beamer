use anyhow::{Context, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    RecordStart,
    RecordStop,
}

pub struct HotkeyHandler {
    manager: GlobalHotKeyManager,
    hotkey_id: u32,
    current_hotkey_str: String,
}

impl HotkeyHandler {
    pub fn new(hotkey_str: &str) -> Result<Self> {
        let manager =
            GlobalHotKeyManager::new().context("Failed to create global hotkey manager")?;

        let hotkey = parse_hotkey(hotkey_str)?;
        let hotkey_id = hotkey.id();
        manager
            .register(hotkey)
            .context("Failed to register global hotkey")?;

        tracing::info!("Registered global hotkey: {}", hotkey_str);

        Ok(Self {
            manager,
            hotkey_id,
            current_hotkey_str: hotkey_str.to_string(),
        })
    }

    /// Start listening for hotkey events. Returns a receiver.
    /// `mode` should be "hold" or "toggle".
    pub fn start_listening(&self, mode: &str) -> mpsc::UnboundedReceiver<HotkeyEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let hotkey_id = self.hotkey_id;
        let is_toggle = mode == "toggle";

        std::thread::spawn(move || {
            let mut recording = false;
            let receiver = GlobalHotKeyEvent::receiver();

            loop {
                if let Ok(event) = receiver.recv() {
                    if event.id() != hotkey_id {
                        continue;
                    }

                    if is_toggle {
                        if event.state() == HotKeyState::Pressed {
                            recording = !recording;
                            let ev = if recording {
                                HotkeyEvent::RecordStart
                            } else {
                                HotkeyEvent::RecordStop
                            };
                            let _ = tx.send(ev);
                        }
                    } else {
                        // Hold mode
                        match event.state() {
                            HotKeyState::Pressed => {
                                let _ = tx.send(HotkeyEvent::RecordStart);
                            }
                            HotKeyState::Released => {
                                let _ = tx.send(HotkeyEvent::RecordStop);
                            }
                        }
                    }
                }
            }
        });

        rx
    }

    pub fn update_hotkey(&mut self, hotkey_str: &str) -> Result<()> {
        // Unregister old, register new
        if let Ok(old) = parse_hotkey(&self.current_hotkey_str) {
            let _ = self.manager.unregister(old);
        }
        let hotkey = parse_hotkey(hotkey_str)?;
        self.hotkey_id = hotkey.id();
        self.current_hotkey_str = hotkey_str.to_string();
        self.manager
            .register(hotkey)
            .context("Failed to register new hotkey")?;
        Ok(())
    }
}

fn parse_hotkey(s: &str) -> Result<HotKey> {
    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
    let mut modifiers = Modifiers::empty();
    let mut key_code = None;

    for part in &parts {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "alt" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            "super" | "win" | "meta" => modifiers |= Modifiers::META,
            key => {
                key_code = Some(parse_key_code(key)?);
            }
        }
    }

    let code = key_code.context("No key code found in hotkey string")?;
    Ok(HotKey::new(Some(modifiers), code))
}

fn parse_key_code(s: &str) -> Result<Code> {
    let code = match s.to_lowercase().as_str() {
        "space" => Code::Space,
        "enter" | "return" => Code::Enter,
        "tab" => Code::Tab,
        "escape" | "esc" => Code::Escape,
        "backspace" => Code::Backspace,
        "delete" => Code::Delete,
        "f1" => Code::F1,
        "f2" => Code::F2,
        "f3" => Code::F3,
        "f4" => Code::F4,
        "f5" => Code::F5,
        "f6" => Code::F6,
        "f7" => Code::F7,
        "f8" => Code::F8,
        "f9" => Code::F9,
        "f10" => Code::F10,
        "f11" => Code::F11,
        "f12" => Code::F12,
        c if c.len() == 1 => {
            let ch = c.chars().next().unwrap();
            match ch {
                'a' => Code::KeyA,
                'b' => Code::KeyB,
                'c' => Code::KeyC,
                'd' => Code::KeyD,
                'e' => Code::KeyE,
                'f' => Code::KeyF,
                'g' => Code::KeyG,
                'h' => Code::KeyH,
                'i' => Code::KeyI,
                'j' => Code::KeyJ,
                'k' => Code::KeyK,
                'l' => Code::KeyL,
                'm' => Code::KeyM,
                'n' => Code::KeyN,
                'o' => Code::KeyO,
                'p' => Code::KeyP,
                'q' => Code::KeyQ,
                'r' => Code::KeyR,
                's' => Code::KeyS,
                't' => Code::KeyT,
                'u' => Code::KeyU,
                'v' => Code::KeyV,
                'w' => Code::KeyW,
                'x' => Code::KeyX,
                'y' => Code::KeyY,
                'z' => Code::KeyZ,
                '0' => Code::Digit0,
                '1' => Code::Digit1,
                '2' => Code::Digit2,
                '3' => Code::Digit3,
                '4' => Code::Digit4,
                '5' => Code::Digit5,
                '6' => Code::Digit6,
                '7' => Code::Digit7,
                '8' => Code::Digit8,
                '9' => Code::Digit9,
                _ => anyhow::bail!("Unknown key: {}", c),
            }
        }
        _ => anyhow::bail!("Unknown key: {}", s),
    };
    Ok(code)
}

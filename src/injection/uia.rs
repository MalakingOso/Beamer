use anyhow::Result;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId,
};
use windows::core::Interface;

pub struct UiaResult {
    pub success: bool,
    pub method: &'static str,
    pub target_info: String,
}

/// Try to inject text using UI Automation SetValue pattern
pub fn try_inject_set_value(text: &str) -> Result<UiaResult> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED).ok();
    }

    let result = unsafe { try_set_value_inner(text) };

    unsafe {
        CoUninitialize();
    }

    result
}

unsafe fn try_set_value_inner(text: &str) -> Result<UiaResult> {
    let automation: IUIAutomation =
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

    let focused = automation.GetFocusedElement()?;

    let name = focused
        .CurrentName()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let class = focused
        .CurrentClassName()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let target_info = format!("name={}, class={}", name, class);

    // Try IValueProvider::SetValue
    let pattern_result = focused.GetCurrentPattern(UIA_ValuePatternId);
    if let Ok(pattern) = pattern_result {
        let value_pattern: std::result::Result<IUIAutomationValuePattern, _> = pattern.cast();
        if let Ok(vp) = value_pattern {
            let bstr = windows::core::BSTR::from(text);
            if vp.SetValue(&bstr).is_ok() {
                return Ok(UiaResult {
                    success: true,
                    method: "UIA SetValue",
                    target_info,
                });
            }
        }
    }

    Ok(UiaResult {
        success: false,
        method: "UIA SetValue",
        target_info,
    })
}

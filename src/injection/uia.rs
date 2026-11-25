#![cfg(target_os = "windows")]

//! UI Automation `SetValue` backend, last in the Windows chain on purpose:
//! `SetValue` replaces the control's *entire* content instead of inserting at
//! the cursor, so dictating into a non-empty field silently wipes it.
//! Callers run it on a `spawn_blocking` thread; each entry point initializes COM itself.

use std::sync::mpsc;
use std::time::Duration;

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId,
};
use windows::core::Interface;

/// Ceiling on cross-process UIA COM calls so a hung foreground app cannot hold
/// `DISPATCH_LOCK` and stall every subsequent dictation indefinitely.
const UIA_CALL_TIMEOUT: Duration = Duration::from_secs(3);

pub struct UiaBackend;

impl InjectionBackend for UiaBackend {
    fn name(&self) -> &'static str {
        "uia"
    }

    fn display_name(&self) -> &'static str {
        "UIA SetValue (accessibility)"
    }

    fn available(&self) -> Result<(), String> {
        // Probe on a fresh MTA thread with a timeout: guarantees COM apartment
        // purity and drops `IUIAutomation` strictly *before* `CoUninitialize`.
        run_mta_with_timeout(|| unsafe {
            let created: windows::core::Result<IUIAutomation> =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
            created.map(|_| ()).map_err(|e| anyhow::anyhow!("UIA unavailable: {e}"))
        })
        .map_err(|e| e.to_string())
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let result = try_inject_set_value(text)?;
        if result.success {
            Ok(InjectionResult {
                method: result.method.to_string(),
                target_info: result.target_info,
            })
        } else {
            anyhow::bail!("UIA SetValue failed for: {}", result.target_info)
        }
    }
}

pub struct UiaResult {
    pub success: bool,
    pub method: &'static str,
    pub target_info: String,
}

/// Run a UIA COM closure on a dedicated MTA thread with a hard timeout.
/// Every COM object created inside `f` is dropped before `CoUninitialize()`,
/// avoiding both use-after-`CoUninitialize` UB and pumpless-STA deadlocks on
/// recycled tokio blocking threads (`RPC_E_CHANGED_MODE`).
fn run_mta_with_timeout<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("beamer-uia".into())
        .spawn(move || {
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let we_initialized = hr.is_ok();
            let result = if we_initialized {
                f()
            } else {
                Err(anyhow::anyhow!("UIA: CoInitializeEx(MTA) failed: {hr:?}"))
            };
            // `f()` has returned and dropped all COM pointers before teardown.
            if we_initialized {
                unsafe { CoUninitialize() };
            }
            let _ = tx.send(result);
        })?;
    match rx.recv_timeout(UIA_CALL_TIMEOUT) {
        Ok(res) => res,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            anyhow::bail!("UIA call timed out after {}s", UIA_CALL_TIMEOUT.as_secs())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            anyhow::bail!("UIA worker thread terminated unexpectedly")
        }
    }
}

/// Inject via UIA `IValueProvider::SetValue` on a bounded MTA worker thread.
pub fn try_inject_set_value(text: &str) -> Result<UiaResult> {
    let text = text.to_string();
    run_mta_with_timeout(move || unsafe { try_set_value_inner(&text) })
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

    let pattern_result = focused.GetCurrentPattern(UIA_ValuePatternId);
    if let Ok(pattern) = pattern_result {
        let value_pattern: std::result::Result<IUIAutomationValuePattern, _> = pattern.cast();
        if let Ok(vp) = value_pattern {
            let bstr = windows::core::BSTR::from(text);
            match vp.SetValue(&bstr) {
                Ok(()) => {
                    return Ok(UiaResult {
                        success: true,
                        method: "UIA SetValue",
                        target_info,
                    });
                }
                Err(e) => {
                    tracing::debug!("UIA: SetValue failed for {}: {:?}", target_info, e);
                }
            }
        } else {
            tracing::debug!("UIA: focused element has no Value pattern ({})", target_info);
        }
    } else {
        tracing::debug!("UIA: no pattern on focused element ({})", target_info);
    }

    Ok(UiaResult {
        success: false,
        method: "UIA SetValue",
        target_info,
    })
}

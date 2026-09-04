#![cfg(target_os = "windows")]

//! Register Beamer's AppUserModelID with Windows via a Start Menu shortcut
//! carrying `System.AppUserModel.ID`. Unpackaged apps need this or toasts are
//! silently suppressed (`Toast::show()` still returns `Ok`). Four places must
//! agree on the string — this property, `WINDOWS_APP_USER_MODEL_ID`, the
//! `Toast::new` app id, `Dioxus.toml`'s identifier — and nothing checks it.

use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING, PROPVARIANT};
use windows::Win32::Foundation::{HANDLE, TRUE};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::{PropVariantChangeType, PVCHF_DEFAULT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT, SLGP_RAWPATH,
};

const SHORTCUT_FILE_NAME: &str = "Beamer.lnk";

/// Create or repair the Start Menu shortcut registering `WINDOWS_APP_USER_MODEL_ID`.
/// Best-effort (warn + swallow). Runs on a `spawn_blocking` thread with COM
/// initialized by the caller (see `app_setup::setup_windows_aumid_shortcut`).
pub fn ensure_shortcut() {
    // S_OK/S_FALSE take an apartment ref we must release; RPC_E_CHANGED_MODE means
    // another apartment owns the thread and we took none.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let we_initialized = hr.is_ok();
    if !we_initialized {
        tracing::debug!(
            "AUMID shortcut: COM already initialized in another apartment ({:?})",
            hr
        );
    }

    if let Err(e) = unsafe { ensure_shortcut_inner() } {
        tracing::warn!(
            "Could not create or verify the Start Menu shortcut that registers Beamer's AppUserModelID: {}",
            e
        );
    }

    if we_initialized {
        unsafe {
            CoUninitialize();
        }
    }
}

unsafe fn ensure_shortcut_inner() -> anyhow::Result<()> {
    let shortcut_path = programs_dir()?.join(SHORTCUT_FILE_NAME);
    let exe_path = std::env::current_exe()?;

    if shortcut_matches(&shortcut_path, &exe_path) {
        tracing::debug!("AUMID shortcut already up to date at {:?}", shortcut_path);
        return Ok(());
    }

    write_shortcut(&shortcut_path, &exe_path)?;
    tracing::info!("Wrote Start Menu shortcut for AUMID registration at {:?}", shortcut_path);
    Ok(())
}

/// Per-user Programs folder via `SHGetKnownFolderPath` (stays correct under
/// folder redirection; a hardcoded `%APPDATA%` path would not).
unsafe fn programs_dir() -> anyhow::Result<PathBuf> {
    let raw = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, HANDLE::default())?;
    let owned = raw.to_string();
    CoTaskMemFree(Some(raw.as_ptr() as *const _));
    Ok(PathBuf::from(owned?))
}

/// True if the shortcut already exists with the right target and AUMID. Skipping
/// the rewrite matters: rewriting every launch churns the entry and resets pin state.
unsafe fn shortcut_matches(shortcut_path: &Path, exe_path: &Path) -> bool {
    if !shortcut_path.exists() {
        return false;
    }
    match read_shortcut(shortcut_path) {
        Ok((target, aumid)) => target == exe_path && aumid == crate::WINDOWS_APP_USER_MODEL_ID,
        Err(e) => {
            tracing::debug!(
                "AUMID shortcut exists but could not be read back ({}); recreating it",
                e
            );
            false
        }
    }
}

/// Read an existing shortcut's target path and `System.AppUserModel.ID`.
unsafe fn read_shortcut(shortcut_path: &Path) -> anyhow::Result<(PathBuf, String)> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    let persist_file: IPersistFile = link.cast()?;
    persist_file.Load(&HSTRING::from(shortcut_path.as_os_str()), STGM_READ)?;

    // SLGP_RAWPATH: literal stored path, no resolution — this is a repair check.
    let mut buf = [0u16; 260];
    link.GetPath(&mut buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let target = PathBuf::from(String::from_utf16_lossy(&buf[..end]));

    let props: IPropertyStore = link.cast()?;
    let propvar = props.GetValue(&PKEY_AppUserModel_ID)?;
    let aumid = windows::core::BSTR::try_from(&propvar)?.to_string();

    Ok((target, aumid))
}

/// `PROPVARIANT` for the AUMID, typed VT_LPWSTR. `PROPVARIANT::from(&str)` gives
/// VT_BSTR, and windows-rs doesn't bind `InitPropVariantFromString`, so convert
/// via `PropVariantChangeType` to the type the toast reader expects.
unsafe fn aumid_propvariant(value: &str) -> anyhow::Result<PROPVARIANT> {
    let bstr_variant = PROPVARIANT::from(value);
    let mut lpwstr_variant = PROPVARIANT::new();
    PropVariantChangeType(&mut lpwstr_variant, &bstr_variant, PVCHF_DEFAULT, VT_LPWSTR)?;
    Ok(lpwstr_variant)
}

/// Build a fresh link at `exe_path`, stamp the AUMID, save to `shortcut_path`.
unsafe fn write_shortcut(shortcut_path: &Path, exe_path: &Path) -> anyhow::Result<()> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    link.SetPath(&HSTRING::from(exe_path.as_os_str()))?;
    link.SetDescription(&HSTRING::from("Beamer"))?;

    // Commit before Save: the property must be in the in-memory link first.
    let props: IPropertyStore = link.cast()?;
    let propvar = aumid_propvariant(crate::WINDOWS_APP_USER_MODEL_ID)?;
    props.SetValue(&PKEY_AppUserModel_ID, &propvar)?;
    props.Commit()?;

    if let Some(parent) = shortcut_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let persist_file: IPersistFile = link.cast()?;
    persist_file.Save(&HSTRING::from(shortcut_path.as_os_str()), TRUE)?;

    Ok(())
}

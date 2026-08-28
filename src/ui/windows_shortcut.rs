#![cfg(target_os = "windows")]

//! Registers Beamer's AppUserModelID (AUMID) with Windows by writing a Start
//! Menu shortcut that carries it as the `System.AppUserModel.ID` property.
//!
//! `SetCurrentProcessExplicitAppUserModelID` (called at startup in `main.rs`)
//! tells Windows what to attribute *this process* to, but for an unpackaged
//! app that AUMID has to already be registered somewhere or toasts are
//! suppressed outright. `Toast::show()` still returns `Ok`, so there is no
//! error for a caller to react to. Windows only learns an AUMID for an
//! unpackaged app from a Start Menu shortcut carrying the property, so this
//! module is what makes the process-wide call actually mean something.
//!
//! Four places have to agree on the same string, and nothing checks that
//! they do: this shortcut's `System.AppUserModel.ID` property (set below),
//! `crate::WINDOWS_APP_USER_MODEL_ID` (the process-wide call in `main.rs`),
//! the `app_id` passed to `Toast::new` in `orchestrator::notify`, and
//! `identifier` in `Dioxus.toml`. An unregistered or mismatched AUMID fails
//! by silence, not by an error any of these call sites can see.

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

/// Create or repair the Start Menu shortcut that registers
/// `crate::WINDOWS_APP_USER_MODEL_ID` with Windows. Best-effort: any failure
/// here only leaves toast branding unregistered, so it is logged at warn and
/// otherwise swallowed rather than surfaced to the caller.
///
/// COM must be initialized on the calling thread before this runs. See
/// `app_setup::setup_windows_aumid_shortcut`, the only caller, which puts it
/// on a `tokio::task::spawn_blocking` thread the same way
/// `injection::uia::try_inject_set_value` does for UIA.
pub fn ensure_shortcut() {
    // Balance mirrors `injection::uia::try_inject_set_value`: S_OK and
    // S_FALSE both take a reference on this thread's apartment that must be
    // released; RPC_E_CHANGED_MODE means a different apartment already owns
    // the thread and no reference was taken, so nothing here should release
    // one that isn't ours.
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

/// The per-user Start Menu Programs folder, resolved through
/// `SHGetKnownFolderPath` rather than a hardcoded `%APPDATA%` path. That
/// folder can be redirected (roaming profiles, group policy), and the
/// known-folder API is what stays correct when it is.
unsafe fn programs_dir() -> anyhow::Result<PathBuf> {
    let raw = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, HANDLE::default())?;
    let owned = raw.to_string();
    CoTaskMemFree(Some(raw.as_ptr() as *const _));
    Ok(PathBuf::from(owned?))
}

/// Cheap check before touching the Start Menu: only true if a shortcut
/// already exists at `shortcut_path`, its target is `exe_path`, and its
/// `System.AppUserModel.ID` already matches. Rewriting the shortcut on every
/// launch would churn the user's Start Menu entry and reset its pin state,
/// so this has to be right before `write_shortcut` runs.
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

    // SLGP_RAWPATH: the literal stored path, no environment-variable
    // resolution or link-target search. This is a repair check, not a
    // launch, so it should see exactly what was written. `pfd` is
    // documented as nullable when the caller doesn't need find data.
    let mut buf = [0u16; 260]; // MAX_PATH; shortcut targets are ordinary file paths
    link.GetPath(&mut buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let target = PathBuf::from(String::from_utf16_lossy(&buf[..end]));

    let props: IPropertyStore = link.cast()?;
    let propvar = props.GetValue(&PKEY_AppUserModel_ID)?;
    let aumid = windows::core::BSTR::try_from(&propvar)?.to_string();

    Ok((target, aumid))
}

/// Build the `PROPVARIANT` for `System.AppUserModel.ID`, typed VT_LPWSTR.
///
/// The obvious route, `PROPVARIANT::from(&str)`, builds VT_BSTR instead:
/// windows-rs 0.58 doesn't bind `InitPropVariantFromString` (it's a header
/// inline in the Windows SDK, not an exported symbol), so there is no direct
/// path to the string PROPVARIANT that Microsoft's own AUMID shortcut
/// samples use. `PropVariantChangeType` does the same conversion that inline
/// function would have, so the property lands on disk as the type Windows'
/// toast-attribution reader expects rather than whatever the read-back
/// happens to tolerate.
unsafe fn aumid_propvariant(value: &str) -> anyhow::Result<PROPVARIANT> {
    let bstr_variant = PROPVARIANT::from(value);
    let mut lpwstr_variant = PROPVARIANT::new();
    PropVariantChangeType(&mut lpwstr_variant, &bstr_variant, PVCHF_DEFAULT, VT_LPWSTR)?;
    Ok(lpwstr_variant)
}

/// Build a fresh `IShellLinkW` pointed at `exe_path`, stamp it with
/// `crate::WINDOWS_APP_USER_MODEL_ID`, and save it to `shortcut_path`.
unsafe fn write_shortcut(shortcut_path: &Path, exe_path: &Path) -> anyhow::Result<()> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    link.SetPath(&HSTRING::from(exe_path.as_os_str()))?;
    link.SetDescription(&HSTRING::from("Beamer"))?;

    // The property has to be committed on the IPropertyStore before the
    // link itself is saved: Commit() flushes it into the in-memory shell
    // link object that IPersistFile::Save then serializes.
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

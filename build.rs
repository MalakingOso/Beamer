fn main() {
    // Embed Windows application manifest for DPI awareness and asInvoker.
    //
    // This has to be a runtime `std::env::var("CARGO_CFG_TARGET_OS")` check,
    // not `#[cfg(target_os = "windows")]` or `cfg!(target_os = "windows")`.
    // Both of those read as obviously correct and are not: build.rs is
    // compiled and run for the *host*, so any `cfg`-based check, including
    // the `cfg!` macro, evaluates against the host's OS. Cross-compiling from
    // Linux to Windows would make the whole block vanish silently: no error,
    // no icon, no manifest, and `winresource` never runs. `CARGO_CFG_TARGET_OS`
    // is the one thing Cargo sets to the *target* triple's OS, which is what
    // this block actually needs to know.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
    </windowsSettings>
  </application>
</assembly>"#,
        );
        res.compile().expect("Failed to compile Windows resources");
    }
}

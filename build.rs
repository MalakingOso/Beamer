use std::io::Write;

fn main() {
    // This has to be a runtime `std::env::var("CARGO_CFG_TARGET_OS")` check,
    // not `#[cfg(target_os = "windows")]` or `cfg!(target_os = "windows")`.
    // Both of those read as obviously correct and are not: build.rs is
    // compiled and run for the *host*, so any `cfg`-based check evaluates
    // against the host's OS. Cross-compiling from Linux to Windows would make
    // the whole block vanish silently, with no error and no manifest.
    // `CARGO_CFG_TARGET_OS` is the one thing Cargo sets to the *target*
    // triple's OS, which is what this block needs to know.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Only the manifest is emitted here, and deliberately not through a
    // resource file.
    //
    // `dx` writes its own Windows resource (icon plus VERSIONINFO, from the
    // `[bundle]` settings in Dioxus.toml) and links it unconditionally, with no
    // opt-out. A second resource carrying its own VERSIONINFO makes the
    // resource compiler fail the whole link:
    //
    //   CVTRES : fatal error CVT1100: duplicate resource. type:VERSION, name:1
    //   LINK : fatal error LNK1123: failure during conversion to COFF
    //
    // `winresource` always emits a VERSIONINFO block and offers no way to skip
    // it, so a resource file here can never coexist with dx's. The manifest is
    // the one thing dx does not provide, and the MSVC linker can embed it
    // directly without a resource, which sidesteps the collision entirely.
    //
    // The icon comes from dx, driven by `icon_path` in Dioxus.toml.
    let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
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
</assembly>"#;

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is always set for a build script");
    let path = std::path::Path::new(&out_dir).join("beamer.manifest");
    let mut file = std::fs::File::create(&path).expect("failed to create the manifest file");
    file.write_all(manifest.as_bytes())
        .expect("failed to write the manifest file");

    // Scoped to the app binary. sync_server is a headless console program and
    // has no use for a DPI or execution-level manifest.
    println!("cargo:rustc-link-arg-bin=beamer=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=beamer=/MANIFESTINPUT:{}",
        path.display()
    );
    println!("cargo:rerun-if-changed=build.rs");
}

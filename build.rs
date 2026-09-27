//! Build script: for Windows targets, embeds an app manifest (asInvoker,
//! PerMonitorV2 DPI awareness) into the `beamer` binary. The icon and
//! VERSIONINFO come from `dx`, via the `[bundle]` settings in Dioxus.toml.

use std::io::Write;

fn main() {
    // Must be a runtime `CARGO_CFG_TARGET_OS` check, not `cfg!`/`#[cfg]`:
    // build.rs runs on the host, so a cfg check silently drops the manifest
    // when cross-compiling from Linux to Windows.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Embedded by the MSVC linker, deliberately not via a resource file: dx
    // always links its own resource with VERSIONINFO, and a second one (which
    // `winresource` always emits) fails the link with
    // `CVT1100: duplicate resource. type:VERSION` / `LNK1123`.
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

    // App binary only: the headless sync_server has no use for a manifest.
    println!("cargo:rustc-link-arg-bin=beamer=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=beamer=/MANIFESTINPUT:{}",
        path.display()
    );
    println!("cargo:rerun-if-changed=build.rs");
}

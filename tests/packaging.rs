//! Pins the release-packaging contract for the GNOME extension sources.
//!
//! `install::gnome_extension::locate_source_dir` probes an FHS path derived
//! from the executable (`<exe>/../share/beamer/extension/<uuid>`). Two
//! install paths must populate it: the .deb (via `[bundle.deb.files]` in
//! `Dioxus.toml`) and `deploy/install-linux.sh`. Dev runs never notice a
//! gap because the `./extension/` fallback resolves from the repo root, so
//! without this test a missing entry only surfaces on a fresh machine.

use std::collections::HashSet;
use std::path::PathBuf;

const UUID: &str = "beamer-focus@beamer.app";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every file in the extension source dir must have a `[bundle.deb.files]`
/// entry whose destination is the FHS share path the app probes. `dx bundle`
/// copies files, not directories, so a file added to the extension dir
/// without an entry here would silently ship a stale helper.
#[test]
fn deb_bundles_every_extension_file() {
    let root = repo_root();
    let src_dir = root.join("extension").join(UUID);
    let mut shipped: HashSet<String> = HashSet::new();
    for entry in std::fs::read_dir(&src_dir).expect("extension source dir readable") {
        let entry = entry.expect("extension dir entry readable");
        if entry.file_type().expect("file type readable").is_file() {
            shipped.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    assert!(
        !shipped.is_empty(),
        "extension source dir unexpectedly empty"
    );

    let manifest =
        std::fs::read_to_string(root.join("Dioxus.toml")).expect("Dioxus.toml readable");
    let parsed: toml::Value = toml::from_str(&manifest).expect("Dioxus.toml parses");
    let files = parsed
        .get("bundle")
        .and_then(|b| b.get("deb"))
        .and_then(|d| d.get("files"))
        .expect("[bundle.deb.files] present in Dioxus.toml");
    // Keys are in-package destinations, values are repo-relative sources
    // (see the working sync_server entry).
    let mut covered: HashSet<String> = HashSet::new();
    for (dest, src) in files.as_table().expect("[bundle.deb.files] is a table") {
        let src = src.as_str().expect("files values are paths");
        let prefix = format!("extension/{UUID}/");
        if let Some(name) = src.strip_prefix(&prefix) {
            assert_eq!(
                *dest,
                format!("usr/share/beamer/extension/{UUID}/{name}"),
                "extension file must map to the FHS share path the app probes"
            );
            covered.insert(name.to_string());
        }
    }
    let missing: Vec<&String> = shipped.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "extension files missing from [bundle.deb.files]: {missing:?}"
    );
}

/// `deploy/install-linux.sh` must copy the extension sources to the same FHS
/// share path, or script-installed users get a dead Install button outside
/// a checkout.
#[test]
fn install_script_ships_extension_sources() {
    let root = repo_root();
    let script = std::fs::read_to_string(root.join("deploy/install-linux.sh"))
        .expect("deploy/install-linux.sh readable");
    assert!(
        script.contains("share/beamer/extension") && script.contains(UUID),
        "install-linux.sh must install the extension sources to the FHS share path"
    );
}

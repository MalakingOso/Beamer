//! Pins the release-packaging contract: the exe is self-contained. A
//! self-update swaps only the binary, so nothing the app needs at runtime may
//! live in a file beside it.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// No manganis `asset!()` in app code: it emits a content-hashed file beside
/// the exe, so an updated binary asks for a file the old install never had
/// (1.0.4 shipped an unstyled main window this way). Embed with
/// `include_str!`/`include_bytes!`. Comment lines are skipped so the reason
/// can still be written next to the code.
#[test]
fn app_source_has_no_manganis_assets() {
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("src dir readable") {
            let path = entry.expect("src entry readable").path();
            if path.is_dir() {
                walk(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("source readable");
                for (i, line) in text.lines().enumerate() {
                    if !line.trim_start().starts_with("//") && line.contains("asset!(") {
                        hits.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
    }
    let mut hits = Vec::new();
    walk(&repo_root().join("src"), &mut hits);
    assert!(
        hits.is_empty(),
        "manganis asset!() breaks exe-only self-updates; embed instead: {hits:?}"
    );
}

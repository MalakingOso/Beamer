//! Download + verify, generalized over (url, sha256, size, destination).
//! Moved out of the old first-run `model_setup` so the model and the runtime
//! archive share one path. No HTTP Range resume: a `.part` is always
//! overwritten from scratch, never appended to.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};

/// `<dest>.part`. The uninstaller deletes the model's by this exact name.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// Stream `url` into `to` (created or truncated), reporting
/// `(received, total)` as it goes. Checks nothing about the content; the
/// caller verifies before trusting it.
pub async fn download(
    url: &str,
    expected_size: u64,
    to: &Path,
    mut progress: impl FnMut(u64, u64),
) -> anyhow::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Shared pool with `llm::chat`. It sets only a connect timeout, never an
    // overall one, so a 1.1 GB body is not cut off mid-stream.
    let client = crate::llm::client::http_client();
    let response = client.get(url).send().await?.error_for_status()?;
    let total = response.content_length().unwrap_or(expected_size);

    let mut file = std::fs::File::create(to)?;
    let mut received: u64 = 0;
    progress(0, total);

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        received += chunk.len() as u64;
        progress(received, total);
    }
    file.flush()?;
    Ok(())
}

/// Size check first (cheap, catches a truncated file without hashing it),
/// then a streamed sha256 over a 64KB buffer so a large file is never read
/// fully into memory. Blocking: 1.1 GB takes seconds, so call it from
/// `spawn_blocking`.
pub fn verify_against(path: &Path, expected_size: u64, expected_sha256: &str) -> anyhow::Result<bool> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() != expected_size {
        return Ok(false);
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = format!("{:x}", hasher.finalize());
    Ok(digest.eq_ignore_ascii_case(expected_sha256))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "beamer-fetch-test-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn sha_of(contents: &[u8]) -> String {
        format!("{:x}", Sha256::digest(contents))
    }

    #[test]
    fn every_catalog_sha256_is_the_right_length() {
        for c in super::super::LLAMA_SERVER {
            if let super::super::Kind::File { sha256, .. } | super::super::Kind::Archive { sha256, .. } = c.kind {
                assert_eq!(sha256.len(), 64, "{}: a sha256 hex digest is exactly 64 characters", c.id);
                assert!(sha256.chars().all(|ch| ch.is_ascii_hexdigit()), "{}: not hex", c.id);
            }
        }
    }

    #[test]
    fn a_file_of_the_wrong_size_fails_before_any_hashing() {
        let path = temp_file("wrong-size.bin", b"not the real model");
        assert!(!verify_against(&path, 999_999, &"0".repeat(64)).unwrap());
    }

    #[test]
    fn a_file_of_the_right_size_but_wrong_content_fails() {
        let contents = b"twelve bytes";
        let path = temp_file("wrong-hash.bin", contents);
        assert!(!verify_against(&path, contents.len() as u64, &"0".repeat(64)).unwrap());
    }

    #[test]
    fn a_matching_size_and_hash_passes() {
        let contents = b"twelve bytes";
        let path = temp_file("matching.bin", contents);
        assert!(verify_against(&path, contents.len() as u64, &sha_of(contents)).unwrap());
    }

    #[test]
    fn hash_comparison_is_case_insensitive() {
        let contents = b"twelve bytes";
        let path = temp_file("case.bin", contents);
        let upper = sha_of(contents).to_ascii_uppercase();
        assert!(verify_against(&path, contents.len() as u64, &upper).unwrap());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_false() {
        let path = std::env::temp_dir().join("beamer-fetch-test-definitely-missing.bin");
        assert!(verify_against(&path, 0, &"0".repeat(64)).is_err());
    }

    #[test]
    fn the_part_path_appends_to_the_full_filename() {
        let dest = Path::new("models").join("K2-Horizon-0.9B-Q8_0.gguf");
        assert_eq!(part_path(&dest), Path::new("models").join("K2-Horizon-0.9B-Q8_0.gguf.part"));
    }
}

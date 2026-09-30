//! Download + sha256 verify for catalog `File` and `Archive` components (the
//! model and the runtime zip share this path). No HTTP Range resume: a
//! `.part` is always overwritten from scratch, never appended to.

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

/// Whether `path` has the expected size and sha256. The size check runs first
/// (cheap, catches truncation), then a streamed hash. Blocking: 1.1 GB takes
/// seconds, so call it from `spawn_blocking`.
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
}

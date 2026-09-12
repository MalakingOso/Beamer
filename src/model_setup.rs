//! First-run setup for K2-Horizon local extraction on a fresh
//! Windows-on-ARM64 install: download the model, verify it, then trigger the
//! pre-registered, dormant Scheduled Task (`hooks.nsh` registers it; this
//! module is the only thing that ever starts it, aside from its own
//! `AtLogOn` trigger on later logins). Beamer otherwise never touches server
//! lifecycle — this is the one narrow, deliberate exception, and it fires at
//! most once per install: `App()` gates the call to [`spawn_ensure_model_present`]
//! on a fresh `config.toml` (see `agent_docs/local_inference.md`).
//!
//! Not under `src/llm/`: this needs [`crate::config::Config`] for paths,
//! which `src/llm/**` may not reference (see
//! `agent_docs/local_inference.md`'s "no crate-rooted paths" section).
//!
//! The module itself compiles on every platform (nothing here is
//! architecture-specific at the type level — `start_server_task` is the one
//! function with real platform behavior, and it's `#[cfg(target_os =
//! "windows")]`-gated internally, a no-op elsewhere). What's actually gated
//! to `aarch64` is the one call site that matters: `App()` only calls
//! [`spawn_ensure_model_present`] on `cfg(target_arch = "aarch64")`, since
//! that's the only arch the bundled `llama-server.exe` fork build exists for
//! (x86_64 Windows and Linux/callisto keep the Gemma default — see
//! `default_extract_model` in `src/llm/mod.rs`). Keeping the gate at the
//! call site rather than around this whole file avoids sprinkling `#[cfg]`
//! through the UI layer that threads `Signal<DownloadStatus>` through props.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use dioxus::prelude::*;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

/// Re-verify against `GET
/// https://huggingface.co/api/models/NANI-Nithin/K2-Horizon-0.9B-GGUF?blobs=true`
/// if this model file is ever swapped for a different quant/version — these
/// three constants must always describe the exact file `MODEL_URL` serves.
const MODEL_URL: &str =
    "https://huggingface.co/NANI-Nithin/K2-Horizon-0.9B-GGUF/resolve/main/K2-Horizon-0.9B-Q8_0.gguf";
const MODEL_SHA256: &str = "741fb9ad263956c003883cb7caddeb14b4dfb9b4b67b5414ad12967e965dd547";
const MODEL_SIZE: u64 = 1_148_614_016;

/// Must match `hooks.nsh`'s `K2H_TASK_NAME` exactly — two independent
/// literals, not a shared constant, since one lives in Rust and the other in
/// an NSIS script with no way to share a value across them.
#[cfg(target_os = "windows")]
const TASK_NAME: &str = "Beamer K2-Horizon Server";

/// Drives the Local AI settings card's progress line.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DownloadStatus {
    #[default]
    Idle,
    Verifying,
    Downloading {
        bytes: u64,
        total: u64,
    },
    Failed(String),
    Ready,
}

/// Same convention as the existing Gemma/S1-mini models
/// (`agent_docs/local_inference.md`), independent of where Beamer itself is
/// installed — see the module doc and `hooks.nsh` for why this must be a
/// fixed path rather than something under the app's own install directory.
fn model_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join("models")
        .join("beamer")
        .join("K2-Horizon-0.9B-Q8_0.gguf")
}

fn part_path() -> PathBuf {
    let mut p = model_path().into_os_string();
    p.push(".part");
    PathBuf::from(p)
}

/// Spawn the fresh-install download-and-start flow as its own Dioxus task
/// (same shape as `notes::pipeline::use_pipeline`'s coroutine). Call only
/// when `App()` has determined this is a fresh install — an upgrade over an
/// existing install must not retroactively trigger this; the file placement
/// and Scheduled Task *update* for that case are instead handled entirely at
/// install time, in `hooks.nsh`.
pub fn spawn_ensure_model_present(mut status: Signal<DownloadStatus>) {
    spawn(async move {
        if let Err(e) = ensure_model_present(&mut status).await {
            tracing::warn!("K2-Horizon model setup failed: {e}");
            status.set(DownloadStatus::Failed(e.to_string()));
        }
    });
}

/// User-triggered retry after a `Failed` status — same flow, callable again
/// from the settings card without waiting for the next app launch.
pub fn retry(status: Signal<DownloadStatus>) {
    spawn_ensure_model_present(status);
}

/// Cancel a download in progress: drop is enough to stop the streaming
/// future (the spawned task simply won't be polled again), but the partial
/// file on disk needs explicit cleanup so a later run doesn't mistake a
/// truncated `.part` for a fresh start it can resume — decision 8 is
/// restart-from-scratch, never resume.
pub fn cancel(mut status: Signal<DownloadStatus>) {
    std::fs::remove_file(part_path()).ok();
    status.set(DownloadStatus::Idle);
}

async fn ensure_model_present(status: &mut Signal<DownloadStatus>) -> anyhow::Result<()> {
    let dest = model_path();

    if dest.exists() {
        status.set(DownloadStatus::Verifying);
        if verify_against(&dest, MODEL_SIZE, MODEL_SHA256)? {
            status.set(DownloadStatus::Ready);
            start_server_task()?;
            return Ok(());
        }
        // Corrupt or partial leftover at the exact target path is a real
        // possibility (e.g. an interrupted prior run), not hypothetical —
        // treat it the same as "missing" rather than trusting it.
        tracing::warn!(
            "existing model at {:?} failed verification, redownloading",
            dest
        );
        std::fs::remove_file(&dest).ok();
    }

    download(&dest, status).await?;

    status.set(DownloadStatus::Verifying);
    if !verify_against(&dest, MODEL_SIZE, MODEL_SHA256)? {
        std::fs::remove_file(&dest).ok();
        anyhow::bail!("downloaded file failed sha256 verification");
    }

    status.set(DownloadStatus::Ready);
    start_server_task()?;
    Ok(())
}

/// Stream `MODEL_URL` to a `.part` file, reporting progress via `status`,
/// then rename onto `dest` only once the whole body has landed. No HTTP
/// Range resume (decision 8): any failure here leaves the `.part` file for
/// the *next* call to overwrite from scratch via `File::create`, not append to.
async fn download(dest: &Path, status: &mut Signal<DownloadStatus>) -> anyhow::Result<()> {
    let tmp = part_path();
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let client = crate::llm::client::http_client();
    let response = client.get(MODEL_URL).send().await?.error_for_status()?;
    let total = response.content_length().unwrap_or(MODEL_SIZE);

    let mut file = std::fs::File::create(&tmp)?;
    let mut received: u64 = 0;
    status.set(DownloadStatus::Downloading { bytes: 0, total });

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        received += chunk.len() as u64;
        status.set(DownloadStatus::Downloading {
            bytes: received,
            total,
        });
    }
    file.flush()?;
    drop(file);

    crate::notes::sync_doc::rename_with_retry(&tmp, dest)?;
    Ok(())
}

/// Size check first (cheap, catches a truncated file without hashing it),
/// then a streamed sha256 over a 64KB buffer so a large file is never read
/// fully into memory. Takes the expected size/hash as parameters, not the
/// module constants directly, so tests can exercise this against a tiny fake
/// file instead of a 1.15GB one.
fn verify_against(path: &Path, expected_size: u64, expected_sha256: &str) -> anyhow::Result<bool> {
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

/// Trigger the pre-registered, dormant Scheduled Task (`hooks.nsh`
/// registers it at install time, without `/run`). Beamer never spawns
/// `llama-server.exe` itself — this shells out to Task Scheduler's own
/// `/run` instead, the one narrow, deliberate exception described in the
/// module doc, and it only ever runs once [`verify_against`] has confirmed
/// the model file is good.
fn start_server_task() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = std::process::Command::new("schtasks")
            .args(["/run", "/tn", TASK_NAME])
            .creation_flags(CREATE_NO_WINDOW)
            .status()?;
        if !status.success() {
            anyhow::bail!("schtasks /run exited with {status}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "beamer-model-setup-test-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn the_published_sha256_is_the_right_length() {
        assert_eq!(
            MODEL_SHA256.len(),
            64,
            "a sha256 hex digest is exactly 64 characters"
        );
    }

    #[test]
    fn a_file_of_the_wrong_size_fails_before_any_hashing() {
        let path = temp_file("wrong-size.bin", b"not the real model");
        assert!(!verify_against(&path, 999_999, MODEL_SHA256).unwrap());
    }

    #[test]
    fn a_file_of_the_right_size_but_wrong_content_fails() {
        let contents = b"twelve bytes";
        let path = temp_file("wrong-hash.bin", contents);
        let bogus_hash = "0".repeat(64);
        assert!(!verify_against(&path, contents.len() as u64, &bogus_hash).unwrap());
    }

    #[test]
    fn a_matching_size_and_hash_passes() {
        let contents = b"twelve bytes";
        let path = temp_file("matching.bin", contents);
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(contents);
            format!("{:x}", hasher.finalize())
        };
        assert!(verify_against(&path, contents.len() as u64, &hash).unwrap());
    }

    #[test]
    fn hash_comparison_is_case_insensitive() {
        let contents = b"twelve bytes";
        let path = temp_file("case.bin", contents);
        let hash_upper = {
            let mut hasher = Sha256::new();
            hasher.update(contents);
            format!("{:X}", hasher.finalize())
        };
        assert!(verify_against(&path, contents.len() as u64, &hash_upper).unwrap());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_false() {
        let path = std::env::temp_dir().join("beamer-model-setup-test-definitely-missing.bin");
        assert!(verify_against(&path, 0, MODEL_SHA256).is_err());
    }
}

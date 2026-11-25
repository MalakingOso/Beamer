//! One-shot warmup of cold-start subsystems (keyring, audio, MPRIS on Linux,
//! network) so the first recording starts without a stall. Run once at launch
//! behind the splash (`ui::app_splash`), which shows `WarmupProgress`.
//! Best-effort: failures are logged and never block startup.

use dioxus::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmupStep {
    Starting,
    Keyring,
    Audio,
    #[cfg(target_os = "linux")]
    Mpris,
    Network,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarmupProgress {
    pub step: WarmupStep,
    pub pct: u8,
}

impl Default for WarmupProgress {
    fn default() -> Self {
        Self {
            step: WarmupStep::Starting,
            pct: 0,
        }
    }
}

/// Ceiling per warmup step so a wedged audio driver or locked keyring can never
/// strand the splash screen on top of the desktop and block `app_ready`.
const STEP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run each warmup step in order, updating `progress` as it goes.
pub async fn warm_all(mut progress: Signal<WarmupProgress>) {

    progress.set(WarmupProgress {
        step: WarmupStep::Keyring,
        pct: 5,
    });
    let t0 = std::time::Instant::now();
    match tokio::time::timeout(
        STEP_TIMEOUT,
        tokio::task::spawn_blocking(|| {
            let _ = crate::config::load_api_key("elevenlabs_api_key");
            let _ = crate::config::load_api_key("mistral_api_key");
        }),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("warmup: keyring step panicked: {}", e),
        Err(_) => tracing::warn!("warmup: keyring step timed out after {:?}", STEP_TIMEOUT),
    }
    tracing::debug!("warmup: keyring took {:?}", t0.elapsed());

    let after_keyring = if cfg!(target_os = "linux") { 25 } else { 33 };
    progress.set(WarmupProgress {
        step: WarmupStep::Audio,
        pct: after_keyring,
    });

    let t1 = std::time::Instant::now();
    match tokio::time::timeout(
        STEP_TIMEOUT,
        tokio::task::spawn_blocking(|| match crate::audio::capture::AudioCapture::new() {
            Ok(_capture) => tracing::debug!("warmup: audio probe ok"),
            Err(e) => tracing::warn!("warmup: audio probe failed: {}", e),
        }),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("warmup: audio step panicked: {}", e),
        Err(_) => tracing::warn!("warmup: audio step timed out after {:?}", STEP_TIMEOUT),
    }
    // Pre-starts the sound-effect thread (WASAPI activation on Windows).
    crate::sounds::warm();
    tracing::debug!("warmup: audio took {:?}", t1.elapsed());

    let after_audio = if cfg!(target_os = "linux") { 50 } else { 66 };

    #[cfg(target_os = "linux")]
    {
        progress.set(WarmupProgress {
            step: WarmupStep::Mpris,
            pct: after_audio,
        });
        let t2 = std::time::Instant::now();
        match tokio::time::timeout(
            STEP_TIMEOUT,
            tokio::task::spawn_blocking(|| match mpris::PlayerFinder::new() {
                Ok(finder) => {
                    let _ = finder.find_all();
                }
                Err(e) => tracing::warn!("warmup: MPRIS finder failed: {}", e),
            }),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("warmup: mpris step panicked: {}", e),
            Err(_) => tracing::warn!("warmup: mpris step timed out after {:?}", STEP_TIMEOUT),
        }
        tracing::debug!("warmup: mpris took {:?}", t2.elapsed());
    }

    let pre_network = if cfg!(target_os = "linux") { 75 } else { after_audio };
    progress.set(WarmupProgress {
        step: WarmupStep::Network,
        pct: pre_network,
    });

    let (backend, api_key) = match tokio::time::timeout(
        STEP_TIMEOUT,
        tokio::task::spawn_blocking(|| {
            let cfg = crate::config::Config::load().unwrap_or_default();
            let backend = cfg.transcription.backend.clone();
            let key_name = match backend.as_str() {
                "voxtral_batch" => "mistral_api_key",
                _ => "elevenlabs_api_key",
            };
            let api_key = crate::config::load_api_key(key_name);
            (backend, api_key)
        }),
    )
    .await
    {
        Ok(Ok(pair)) => pair,
        _ => (String::new(), String::new()),
    };

    if api_key.is_empty() {
        tracing::debug!("warmup: skipping network preconnect — no key for backend '{}'", backend);
    } else {
        let t3 = std::time::Instant::now();
        // Batch-only: an unauthenticated GET warms DNS/TLS with no billable
        // side effect. Unknown backends still preconnect to ElevenLabs; the
        // orchestrator rejects them at record time.
        let result: anyhow::Result<()> = crate::transcription::preconnect_batch_host(&backend).await;
        match result {
            Ok(()) => tracing::debug!("warmup: network preconnect ok ({:?})", t3.elapsed()),
            Err(e) => tracing::warn!("warmup: network preconnect failed: {}", e),
        }
    }

    progress.set(WarmupProgress {
        step: WarmupStep::Done,
        pct: 100,
    });
}

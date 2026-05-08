//! One-shot warmup of cold-start subsystems so the first recording feels instant.
//!
//! On a fresh launch, several things in `orchestrator::handle_recording` pay
//! one-time costs that block the single-threaded executor (DNS, TLS, libsecret
//! D-Bus, cpal host init, MPRIS bus, WebSocket handshake). We pay them up-front
//! behind a splash window so the user doesn't experience a delayed first
//! transcript.

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

/// Run each warmup step sequentially, updating `progress` between steps.
/// Best-effort: any failure is logged and ignored — warmup must never block startup.
pub async fn warm_all(mut progress: Signal<WarmupProgress>) {
    // Step pcts are chosen so each step's *start* is logged at a value < its end,
    // and Done is exactly 100. Linux has 4 steps (25/50/75/100), Windows 3 (33/66/100).

    progress.set(WarmupProgress {
        step: WarmupStep::Keyring,
        pct: 5,
    });
    let t0 = std::time::Instant::now();
    if let Err(e) = tokio::task::spawn_blocking(|| {
        let _ = crate::config::load_api_key("elevenlabs_api_key");
        let _ = crate::config::load_api_key("mistral_api_key");
    })
    .await
    {
        tracing::warn!("warmup: keyring step panicked: {}", e);
    }
    tracing::debug!("warmup: keyring took {:?}", t0.elapsed());

    let after_keyring = if cfg!(target_os = "linux") { 25 } else { 33 };
    progress.set(WarmupProgress {
        step: WarmupStep::Audio,
        pct: after_keyring,
    });

    let t1 = std::time::Instant::now();
    if let Err(e) = tokio::task::spawn_blocking(|| match crate::audio::capture::AudioCapture::new() {
        Ok(_capture) => tracing::debug!("warmup: audio probe ok"),
        Err(e) => tracing::warn!("warmup: audio probe failed: {}", e),
    })
    .await
    {
        tracing::warn!("warmup: audio step panicked: {}", e);
    }
    tracing::debug!("warmup: audio took {:?}", t1.elapsed());

    let after_audio = if cfg!(target_os = "linux") { 50 } else { 66 };

    #[cfg(target_os = "linux")]
    {
        progress.set(WarmupProgress {
            step: WarmupStep::Mpris,
            pct: after_audio,
        });
        let t2 = std::time::Instant::now();
        if let Err(e) = tokio::task::spawn_blocking(|| match mpris::PlayerFinder::new() {
            Ok(finder) => {
                let _ = finder.find_all();
            }
            Err(e) => tracing::warn!("warmup: MPRIS finder failed: {}", e),
        })
        .await
        {
            tracing::warn!("warmup: mpris step panicked: {}", e);
        }
        tracing::debug!("warmup: mpris took {:?}", t2.elapsed());
    }

    let pre_network = if cfg!(target_os = "linux") { 75 } else { after_audio };
    progress.set(WarmupProgress {
        step: WarmupStep::Network,
        pct: pre_network,
    });

    let cfg = crate::config::Config::load().unwrap_or_default();
    let backend = cfg.transcription.backend.clone();
    let language = cfg.transcription.language.clone();
    let key_name = match backend.as_str() {
        "voxtral" | "voxtral_batch" => "mistral_api_key",
        _ => "elevenlabs_api_key",
    };
    let api_key = crate::config::load_api_key(key_name);

    if api_key.is_empty() {
        tracing::debug!("warmup: skipping network preconnect — no key for backend '{}'", backend);
    } else {
        let t3 = std::time::Instant::now();
        let result: anyhow::Result<()> = match backend.as_str() {
            "voxtral" | "voxtral_batch" => {
                match crate::transcription::start_voxtral_session(&api_key).await {
                    Ok(session) => {
                        drop(session);
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            _ => match crate::transcription::start_elevenlabs_session(&api_key, &language).await {
                Ok(session) => {
                    drop(session);
                    Ok(())
                }
                Err(e) => Err(e),
            },
        };
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

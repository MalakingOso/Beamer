use anyhow::Result;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_OWNER: &str = "MalakingOso";
const GITHUB_REPO: &str = "Beamer";
const BINARY_NAME: &str = "beamer";

/// State of the update lifecycle, driven by UI actions and background checks.
#[derive(Clone, PartialEq, Default)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Available { version: String },
    Downloading,
    ReadyToRestart,
    Error(String),
}

pub struct UpdateInfo {
    pub version: String,
}

/// Check GitHub Releases for a newer version.
/// **Blocking** — must be called via `tokio::task::spawn_blocking`.
pub fn check_for_update_blocking() -> Result<Option<UpdateInfo>> {
    let updater = self_update::backends::github::Update::configure()
        .repo_owner(GITHUB_OWNER)
        .repo_name(GITHUB_REPO)
        .bin_name(BINARY_NAME)
        .current_version(CURRENT_VERSION)
        .build()?;

    let latest = updater.get_latest_release()?;
    let latest_version = latest.version.trim_start_matches('v');

    if self_update::version::bump_is_greater(CURRENT_VERSION, latest_version)? {
        Ok(Some(UpdateInfo {
            version: latest_version.to_string(),
        }))
    } else {
        Ok(None)
    }
}

/// Download the latest release and replace the running binary.
/// **Blocking** — must be called via `tokio::task::spawn_blocking`.
pub fn apply_update_blocking() -> Result<()> {
    let status = self_update::backends::github::Update::configure()
        .repo_owner(GITHUB_OWNER)
        .repo_name(GITHUB_REPO)
        .bin_name(BINARY_NAME)
        .current_version(CURRENT_VERSION)
        .no_confirm(true)
        .build()?
        .update()?;

    tracing::info!("Updated to version {}", status.version());
    Ok(())
}

/// Spawn the updated binary and exit the current process.
pub fn restart_app() -> ! {
    let exe = std::env::current_exe().expect("Failed to get current exe path");
    let _ = std::process::Command::new(exe).spawn();
    std::process::exit(0);
}

pub fn current_version() -> &'static str {
    CURRENT_VERSION
}

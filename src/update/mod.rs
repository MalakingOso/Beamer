use anyhow::Result;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_OWNER: &str = "MalakingOso";
const GITHUB_REPO: &str = "Beamer";
const BINARY_NAME: &str = "beamer";

/// Update lifecycle state.
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

/// Check GitHub Releases for a newer version. Blocking: use `spawn_blocking`.
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

/// Download the latest release and replace the running binary. Blocking: use `spawn_blocking`.
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

/// Spawn the updated binary and exit. Releases the single-instance guard first
/// so the child doesn't see this process as a rival instance and quit.
///
/// Like the tray Quit handler this `process::exit`s, skipping destructors and
/// the flush tick — callers must flush the note stores first.
pub fn restart_app() -> ! {
    // Not `current_exe()`: on Linux the update just renamed a new binary over
    // this process's own path, and a fresh query resolves to the old,
    // now-unlinked inode (`<path> (deleted)`). Use the path cached at launch.
    let exe = crate::launch_exe_path();
    crate::release_single_instance();
    match std::process::Command::new(exe).spawn() {
        Ok(child) => tracing::info!("Relaunched Beamer as pid {}", child.id()),
        Err(e) => tracing::error!("Failed to relaunch {:?}: {}", exe, e),
    }
    std::process::exit(0);
}

pub fn current_version() -> &'static str {
    CURRENT_VERSION
}

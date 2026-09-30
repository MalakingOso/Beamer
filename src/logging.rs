//! Process logging: stdout plus `beamer.log` beside `config.toml`. A Windows
//! release build has no console, so without the file every `tracing` line
//! would vanish. The previous run's log is kept as `beamer.log.1`.
//!
//! `RUST_LOG` wins when set; otherwise Settings → Debug → "Debug logging"
//! switches `beamer` between info and debug at runtime.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tracing_subscriber::{fmt, prelude::*, reload, EnvFilter, Registry};

static FILTER: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
/// `RUST_LOG` chose the filter, so the Settings toggle leaves it alone.
static FROM_ENV: AtomicBool = AtomicBool::new(false);

fn level_filter(debug: bool) -> EnvFilter {
    EnvFilter::new(if debug { "beamer=debug" } else { "beamer=info" })
}

/// Rotate the last run's log aside and open a fresh one. `None` (stdout only)
/// if the directory can't be written.
fn open_log_file() -> Option<std::fs::File> {
    let dir = crate::config::Config::config_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("beamer.log");
    let _ = std::fs::rename(&path, dir.join("beamer.log.1"));
    std::fs::File::create(&path).ok()
}

/// Install the global subscriber and the panic hook. Call once the single-
/// instance guard is held, never before: it rotates the log file.
pub fn init() {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => {
            FROM_ENV.store(true, Ordering::Relaxed);
            filter
        }
        Err(_) => level_filter(false),
    };
    let (filter, handle) = reload::Layer::new(filter);
    let file_layer = open_log_file()
        .map(|file| fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(file)));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(file_layer)
        .init();
    let _ = FILTER.set(handle);

    install_panic_hook();
}

/// Apply the `debug_logging` setting (startup and on toggle).
pub fn set_debug(debug: bool) {
    if FROM_ENV.load(Ordering::Relaxed) {
        return;
    }
    if let Some(handle) = FILTER.get() {
        if let Err(e) = handle.reload(level_filter(debug)) {
            tracing::warn!("Could not change the log level: {e}");
        }
    }
}

/// Log panics (message, location, backtrace) before the default hook runs, so
/// a crash leaves a trace in `beamer.log` rather than a vanished tray icon.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic: {info}\n{backtrace}");
        default_hook(info);
    }));
}

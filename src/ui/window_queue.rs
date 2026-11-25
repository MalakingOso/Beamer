//! Global serialization for `DesktopContext::new_window` calls.
//!
//! On Windows, `dioxus-desktop`'s `handle_new_window` holds a `RefMut` borrow
//! on `pending_webviews` while creating each WebView2 instance, and WebView2
//! creation runs a nested Win32 message pump (`wait_with_pump`). Concurrent
//! `new_window` calls — whether at startup (splash + pill) or at runtime
//! (sticky notes + day-end card) — can trigger `WebView2Error(0x8007139F)` or
//! a `RefCell already borrowed` panic. Routing every `new_window` through this
//! async gate guarantees at most one window is under construction at a time.

use std::sync::OnceLock;

use dioxus::desktop::{Config as DesktopConfig, DesktopContext};
use dioxus::prelude::VirtualDom;
use tokio::sync::Mutex;

static WINDOW_OPEN_GATE: OnceLock<Mutex<()>> = OnceLock::new();

fn gate() -> &'static Mutex<()> {
    WINDOW_OPEN_GATE.get_or_init(|| Mutex::new(()))
}

/// Open a new Dioxus desktop window, serialized process-wide against every
/// other `open_window` caller.
pub(super) async fn open_window(
    window: &DesktopContext,
    dom: VirtualDom,
    cfg: DesktopConfig,
) -> DesktopContext {
    let _guard = gate().lock().await;
    window.new_window(dom, cfg).await
}

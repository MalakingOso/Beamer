//! Where sticky notes are allowed to go: the desktop's usable rectangle, the
//! main window's estimated footprint, and this launch's scatter seed.
//!
//! Split from `sticky_windows` to keep that file inside the 500-line limit, and
//! because these are a different kind of thing: `sticky_windows` reconciles a
//! set of windows against a set of notes, while this asks the windowing system
//! where the screens are. It cannot live in `note_layout` either — that module's
//! contract is to be pure and Dioxus-free, and everything here needs a
//! `DesktopContext`.
//!
//! ⚠️ Two traps live here, both measured on 2026-08-25 against a two-monitor
//! GNOME 50 Wayland session, and both silent:
//!
//! - **`primary_monitor()` returns `None` here** while `available_monitors()`
//!   enumerates both screens correctly. Asking only for the primary monitor is
//!   why `work_area` used to fall through to its 1920x1080 fallback on every
//!   launch, confining every note to a seventh of the desktop.
//! - **`MonitorHandle::size()` and `position()` are physical pixels** while
//!   GNOME's `move_frame` takes logical stage coordinates. The monitors here
//!   report 5120x2880 at x=0 and x=5120 with scale 2, so the second monitor's
//!   logical origin is 2560 — miss the conversion and every note on it is
//!   placed a full screen too far right.

use dioxus::desktop::DesktopContext;

use crate::ui::note_layout::Rect;

/// Vertical space reserved for the GNOME top panel.
///
/// A `GetWorkArea` extension method would be more correct — it would account
/// for docks and any other struts — but it is speculative, and adding it later
/// costs only the log out that any other extension change costs anyway.
///
/// Windows-only note: this constant is a GNOME concept and must never apply
/// there. Windows gets its own taskbar-aware rectangle per monitor (see
/// `windows_work_rect`), and `work_area` compensates `union_work_area`'s
/// unconditional use of this constant back out on that target.
const PANEL_INSET: i32 = 40;

/// A plain 1080p desktop, assumed when no monitor can be enumerated.
///
/// Being wrong here costs a badly placed note, not a lost one — `place_next`
/// keeps every note inside whatever rectangle it is given, so the worst case is
/// notes crowded into the top-left of a larger desktop.
fn fallback_work_area() -> Rect {
    Rect { x: 0, y: PANEL_INSET, w: 1920, h: 1080 - PANEL_INSET as u32 }
}

/// Combine already-logical monitor rectangles into the region notes may use.
///
/// Split out from `work_area` because the interesting part — the union, the
/// inset, and the empty case — is arithmetic that needs no windowing system,
/// while the part that fetches the monitors cannot be tested at all.
///
/// The union spans the *gaps* between monitors too. On a mismatched or
/// staggered arrangement that means a note can be told to go somewhere no
/// monitor covers, and Mutter will clamp it back onto one. Modelling the exact
/// shape of a multi-monitor desktop is a great deal of work to avoid an
/// outcome the compositor already handles.
fn union_work_area(monitors: &[Rect]) -> Rect {
    let Some((first, rest)) = monitors.split_first() else {
        return fallback_work_area();
    };
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x + first.w as i32;
    let mut max_y = first.y + first.h as i32;
    for m in rest {
        min_x = min_x.min(m.x);
        min_y = min_y.min(m.y);
        max_x = max_x.max(m.x + m.w as i32);
        max_y = max_y.max(m.y + m.h as i32);
    }
    // The panel is reserved on the top edge of the whole desktop rather than
    // per monitor: GNOME draws one panel, on one monitor, and a note pushed
    // down on every monitor to account for it would waste a strip that is not
    // actually occupied.
    Rect {
        x: min_x,
        y: min_y + PANEL_INSET,
        w: (max_x - min_x).max(0) as u32,
        h: ((max_y - min_y).max(0) as u32).saturating_sub(PANEL_INSET as u32),
    }
}

/// The region notes may be placed in, in the compositor's logical coordinates.
///
/// **Every** monitor contributes, not just the primary one. Notes are ordinary
/// windows in the normal stacking order — they are allowed to sit behind the
/// main window — so the only real defence against a note being lost under
/// something is to spread them out, and confining the scatter to one monitor
/// throws away most of the room available to do that with.
///
/// `MonitorHandle::size()` reports **physical** pixels while GNOME's
/// `move_frame` works in **logical** stage coordinates. The two agree at scale
/// 1.0 and diverge at every other scale, so the factor is divided out here —
/// per monitor, using that monitor's own scale, since a mixed-DPI desktop has
/// no single factor to divide by.
///
/// ⚠️ That per-monitor division is also why mixed-DPI multi-monitor placement
/// stays wrong on Windows even after this function accounts for the taskbar.
/// There is no global logical coordinate space to place a note in: each
/// monitor's physical rectangle is divided by *its own* scale factor and the
/// results are unioned as if they shared one coordinate system, which is only
/// actually true when every monitor uses the same scale. A uniform-DPI desktop
/// (one display, or several matched ones) round-trips correctly; a mismatched
/// pair does not. Modelling Windows' real per-monitor virtual-desktop layout
/// is a bigger change than this fix, and is not attempted here.
///
/// ⚠️ A second, separate gap on multi-monitor Windows: `windows_work_rect`
/// gets a real taskbar-aware rectangle per monitor, but those rectangles are
/// still bounding-boxed together by the shared `union_work_area`. If one
/// monitor is taller than another, or only one of them carries the taskbar,
/// the union's bottom edge extends past the taskbar-bearing monitor's
/// `rcWork` bottom, so a note can still be placed under the taskbar on that
/// monitor. The single-monitor case is genuinely fixed; a mismatched pair is
/// not. The Windows fallback rectangle (`fallback_work_area`, used when no
/// monitor enumerates at all) has the same gap for the same reason: it is a
/// full-resolution guess with no taskbar excluded. Fixing this would mean
/// placing notes per-monitor-rectangle instead of against one shared union,
/// which is the restructure `union_work_area`'s own doc comment already
/// argues against taking on for a gap the compositor (or, here, the user's
/// own repositioning) already handles reasonably.
pub fn work_area(window: &DesktopContext) -> Rect {
    let mut logical: Vec<Rect> = Vec::new();
    for monitor in window.available_monitors() {
        let scale = monitor.scale_factor();

        #[cfg(target_os = "windows")]
        let (origin_x, origin_y, phys_w, phys_h) =
            windows_work_rect(&monitor).unwrap_or_else(|| physical_monitor_rect(&monitor));
        #[cfg(not(target_os = "windows"))]
        let (origin_x, origin_y, phys_w, phys_h) = physical_monitor_rect(&monitor);

        let rect = Rect {
            x: (f64::from(origin_x) / scale) as i32,
            y: (f64::from(origin_y) / scale) as i32,
            w: (f64::from(phys_w) / scale) as u32,
            h: (f64::from(phys_h) / scale) as u32,
        };
        tracing::info!(
            "Monitor {:?}: physical {}x{} at ({}, {}), scale {} -> logical {}x{} at ({}, {})",
            monitor.name().unwrap_or_default(),
            phys_w,
            phys_h,
            origin_x,
            origin_y,
            scale,
            rect.w,
            rect.h,
            rect.x,
            rect.y,
        );
        logical.push(rect);
    }

    // `union_work_area` always reserves `PANEL_INSET` on the top edge for the
    // GNOME panel, and that logic is shared and stays that way (see its own
    // doc comment). Windows has no such panel: `windows_work_rect` above
    // already excludes the taskbar per monitor via `rcWork`, so applying the
    // GNOME inset on top of that would double-reserve space nothing occupies.
    // `undo_gnome_inset` cancels the fixed inset back out on that target
    // rather than forking `union_work_area`, keeping the tested union math in
    // one place.
    let area = undo_gnome_inset(union_work_area(&logical));

    // At info!, not debug!. A wrong work area misplaces every note at once and
    // is otherwise indistinguishable from the scatter being broken, so the
    // rectangle actually used has to be readable without a special log level.
    tracing::info!(
        "Note work area: {}x{} at ({}, {}) from {} monitor(s)",
        area.w,
        area.h,
        area.x,
        area.y,
        logical.len(),
    );
    area
}

/// Cancel `union_work_area`'s fixed GNOME-panel inset back out on Windows,
/// where `windows_work_rect` already produced a taskbar-aware rectangle and
/// no further reservation is wanted. A no-op everywhere else.
#[cfg(target_os = "windows")]
fn undo_gnome_inset(mut area: Rect) -> Rect {
    area.y -= PANEL_INSET;
    area.h += PANEL_INSET as u32;
    area
}

#[cfg(not(target_os = "windows"))]
fn undo_gnome_inset(area: Rect) -> Rect {
    area
}

/// A monitor's full physical rectangle (position and size), the fallback used
/// when a taskbar-aware rectangle either isn't available (non-Windows) or
/// couldn't be read (`GetMonitorInfoW` failing on Windows).
fn physical_monitor_rect(monitor: &dioxus::desktop::tao::monitor::MonitorHandle) -> (i32, i32, u32, u32) {
    let size = monitor.size();
    let origin = monitor.position();
    (origin.x, origin.y, size.width, size.height)
}

/// A monitor's usable rectangle (excluding the taskbar and any other appbar),
/// in the same physical-pixel space as `physical_monitor_rect`.
///
/// `None` means `GetMonitorInfoW` failed for this monitor (or tao's
/// `hmonitor()` doesn't correspond to a monitor Win32 still knows about,
/// possible on a hot-unplug race). Callers fall back to the full rectangle.
#[cfg(target_os = "windows")]
fn windows_work_rect(
    monitor: &dioxus::desktop::tao::monitor::MonitorHandle,
) -> Option<(i32, i32, u32, u32)> {
    use dioxus::desktop::tao::platform::windows::MonitorHandleExtWindows;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};

    let hmonitor = HMONITOR(monitor.hmonitor() as *mut std::ffi::c_void);
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info) };
    if !ok.as_bool() {
        return None;
    }
    let rc = info.rcWork;
    Some((rc.left, rc.top, (rc.right - rc.left).max(0) as u32, (rc.bottom - rc.top).max(0) as u32))
}

/// Logical size the main window is built with — see `ui::launch_app`.
const MAIN_WINDOW_SIZE: (u32, u32) = (500, 600);

/// Points a note should keep clear of so it does not open behind the main
/// window.
///
/// This is an **estimate**, deliberately. The main window is built with a size
/// but no position, so Mutter centres it; Beamer cannot then read where it
/// actually went — `outer_position()` returns a cached `(0, 0)` under Wayland
/// (see `shell_window`), and the extension's title lookup would be ambiguous
/// because the splash window is titled "Beamer" too. Guessing the centre of the
/// primary monitor costs nothing and is right in the ordinary case; being wrong
/// only scatters notes around a patch of empty desktop.
///
/// Returns the rectangle's corners and centre rather than one point: scoring
/// compares top-left corners, so a single point would let a note tuck itself
/// against the far side of the window and still score as clear.
pub fn main_window_points(window: &DesktopContext) -> Vec<(i32, i32)> {
    // ⚠️ `primary_monitor()` returns `None` under Wayland on this GNOME
    // session, while `available_monitors()` enumerates both screens correctly —
    // measured 2026-08-25. Falling back to the first enumerated monitor is not
    // defensive coding for an unlikely case; it is the *normal* path here, and
    // asking only for the primary monitor is why `work_area` used to silently
    // use its 1920x1080 fallback on a 5120x1400 desktop.
    let Some(monitor) = window.primary_monitor().or_else(|| window.available_monitors().next())
    else {
        // Notes will still be scattered, but nothing is keeping them off the
        // main window, and from the outside that is indistinguishable from the
        // avoidance not working.
        tracing::warn!("No monitor to locate the main window on — notes will not avoid it");
        return Vec::new();
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let origin = monitor.position();
    let mx = (f64::from(origin.x) / scale) as i32;
    let my = (f64::from(origin.y) / scale) as i32;
    let mw = (f64::from(size.width) / scale) as i32;
    let mh = (f64::from(size.height) / scale) as i32;

    let (w, h) = (MAIN_WINDOW_SIZE.0 as i32, MAIN_WINDOW_SIZE.1 as i32);
    let left = mx + (mw - w) / 2;
    let top = my + (mh - h) / 2;
    tracing::info!(
        "Keeping notes clear of the main window, estimated at {}x{} centred on {:?} => ({}, {})",
        w,
        h,
        monitor.name().unwrap_or_default(),
        left,
        top,
    );
    vec![
        (left, top),
        (left + w, top),
        (left, top + h),
        (left + w, top + h),
        (left + w / 2, top + h / 2),
    ]
}

/// A seed for this reconcile pass's scatter.
///
/// Nanoseconds rather than seconds: two launches in the same second must not
/// share a layout, and the seconds field alone changes too slowly to guarantee
/// that. Folding the seconds in as well keeps successive passes within one
/// second apart. A clock before the epoch is not worth a branch — the layout
/// merely repeats.
pub fn launch_seed() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ (d.as_secs() as u32).wrapping_mul(2_654_435_761))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_side_by_side_monitors_become_one_wide_work_area() {
        // The real desktop this was widened for: two 4K panels at scale 1.5,
        // i.e. 2560x1440 logical each. Using only the primary one left notes
        // scattered across under a quarter of the available space.
        let left = Rect { x: 0, y: 0, w: 2560, h: 1440 };
        let right = Rect { x: 2560, y: 0, w: 2560, h: 1440 };
        let area = union_work_area(&[left, right]);
        assert_eq!(area.x, 0);
        assert_eq!(area.w, 5120, "the union must span both monitors");
        assert_eq!(area.y, PANEL_INSET, "the panel is reserved on the top edge");
        assert_eq!(area.h, 1440 - PANEL_INSET as u32);
    }

    #[test]
    fn a_monitor_left_of_the_origin_moves_the_work_areas_corner() {
        // Nothing guarantees the primary monitor is the leftmost one. If the
        // union kept x at 0 the whole left-hand monitor would be unreachable.
        let secondary = Rect { x: -1920, y: 0, w: 1920, h: 1080 };
        let primary = Rect { x: 0, y: 0, w: 2560, h: 1440 };
        let area = union_work_area(&[primary, secondary]);
        assert_eq!(area.x, -1920);
        assert_eq!(area.w, 4480);
    }

    #[test]
    fn a_stacked_pair_reserves_the_panel_only_once() {
        // Monitors above one another: the inset belongs to the top edge of the
        // desktop, not to each monitor, or the lower screen loses a strip that
        // no panel occupies.
        let top = Rect { x: 0, y: 0, w: 2560, h: 1440 };
        let bottom = Rect { x: 0, y: 1440, w: 2560, h: 1440 };
        let area = union_work_area(&[top, bottom]);
        assert_eq!(area.h, 2880 - PANEL_INSET as u32);
    }

    #[test]
    fn no_monitors_at_all_falls_back_rather_than_producing_an_empty_area() {
        // A zero-sized work area would send every note to a single corner.
        let area = union_work_area(&[]);
        assert_eq!(area, fallback_work_area());
        assert!(area.w > 0 && area.h > 0);
    }
}

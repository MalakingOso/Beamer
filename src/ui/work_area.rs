//! Note-allowed region: desktop usable rect, main-window keep-clear points, and
//! the launch scatter seed. Split from `sticky_windows` (windowing queries, not
//! reconciliation; `note_layout` must stay pure and Dioxus-free). ⚠️ Two silent
//! traps: `primary_monitor()` returns `None` on this GNOME/Wayland session while
//! `available_monitors()` works — never ask only for primary. And
//! `MonitorHandle` geometry is physical px while `move_frame` takes logical —
//! always divide by the monitor's own scale.

use dioxus::desktop::DesktopContext;

use crate::ui::note_layout::Rect;

/// GNOME top panel reservation. GNOME-only: Windows already excludes its taskbar
/// per monitor (`windows_work_rect`), so `work_area` cancels this back out there.
const PANEL_INSET: i32 = 40;

/// 1080p fallback when no monitor enumerates. Wrong-but-safe: `place_next`
/// keeps notes inside whatever rect it's given.
fn fallback_work_area() -> Rect {
    Rect { x: 0, y: PANEL_INSET, w: 1920, h: 1080 - PANEL_INSET as u32 }
}

/// Union already-logical monitor rects into the note region. Split out: pure
/// arithmetic, testable without a windowing system. The union spans inter-monitor
/// gaps too — Mutter clamps those back, which beats modelling exact desktop shapes.
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
    // One panel on one monitor: reserve on the desktop's top edge, not per monitor.
    Rect {
        x: min_x,
        y: min_y + PANEL_INSET,
        w: (max_x - min_x).max(0) as u32,
        h: ((max_y - min_y).max(0) as u32).saturating_sub(PANEL_INSET as u32),
    }
}

/// Note region in compositor logical coords. Every monitor contributes, and each
/// monitor's physical geometry is divided by its own scale (they agree only at
/// 1x). ⚠️ Known gaps: mixed-DPI unions pretend per-monitor logical rects share
/// one space (true only at uniform DPI), and the shared union can extend past a
/// shorter/taskbar-bearing monitor's usable bottom on Windows.
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

    // Windows has no GNOME panel (`rcWork` already excludes the taskbar), so cancel
    // the shared inset there rather than forking the tested union math.
    let area = undo_gnome_inset(union_work_area(&logical));

    // info, not debug: a wrong area misplaces every note and mimics a scatter bug.
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

/// Cancel the GNOME-panel inset on Windows (`rcWork` already excludes the taskbar).
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

/// A monitor's full physical rect; fallback when no taskbar-aware rect is available.
fn physical_monitor_rect(monitor: &dioxus::desktop::tao::monitor::MonitorHandle) -> (i32, i32, u32, u32) {
    let size = monitor.size();
    let origin = monitor.position();
    (origin.x, origin.y, size.width, size.height)
}

/// Usable rect minus taskbar/appbars (physical px). `None` on `GetMonitorInfoW`
/// failure (e.g. hot-unplug race); callers fall back to the full rect.
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

/// Keep-clear points so notes don't open behind the main window. An estimate by
/// design: the main window is centred by Mutter at an unreadable position
/// (`outer_position()` lies under Wayland; title lookup is ambiguous with the
/// splash), so guess the primary monitor's centre — wrong only scatters around
/// empty desktop. Corners + centre, not one point (scoring compares corners).
pub fn main_window_points(window: &DesktopContext) -> Vec<(i32, i32)> {
    // `primary_monitor()` is `None` on this GNOME/Wayland session — the fallback
    // to first-enumerated is the normal path, not defensive coding.
    let Some(monitor) = window.primary_monitor().or_else(|| window.available_monitors().next())
    else {
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

/// Seed for this pass's scatter. Nanoseconds (not seconds) so two launches in
/// the same second don't share a layout; pre-epoch clocks just repeat it.
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
        let secondary = Rect { x: -1920, y: 0, w: 1920, h: 1080 };
        let primary = Rect { x: 0, y: 0, w: 2560, h: 1440 };
        let area = union_work_area(&[primary, secondary]);
        assert_eq!(area.x, -1920);
        assert_eq!(area.w, 4480);
    }

    #[test]
    fn a_stacked_pair_reserves_the_panel_only_once() {
        let top = Rect { x: 0, y: 0, w: 2560, h: 1440 };
        let bottom = Rect { x: 0, y: 1440, w: 2560, h: 1440 };
        let area = union_work_area(&[top, bottom]);
        assert_eq!(area.h, 2880 - PANEL_INSET as u32);
    }

    #[test]
    fn no_monitors_at_all_falls_back_rather_than_producing_an_empty_area() {
        let area = union_work_area(&[]);
        assert_eq!(area, fallback_work_area());
        assert!(area.w > 0 && area.h > 0);
    }
}

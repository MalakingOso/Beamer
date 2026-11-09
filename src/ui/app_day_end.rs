//! Hook that shows the day-end card (`ui::day_end`) at the time set in
//! `[day_end]`, and on "Show it now" from Settings. Called once from `App()`'s
//! render body like the other `setup_*` hooks (fixed hook order; see
//! `agent_docs/dioxus_architecture.md`).
//!
//! The timer is a chunked loop, not an abort-and-respawn: each pass re-reads
//! the config and sleeps at most a minute, so a Settings change takes effect
//! within one without any effect plumbing, and a machine that slept through the
//! set time shows the card on wake or skips the day (`notes::day_end::GRACE`).
//!
//! Windows/macOS get the card window. Linux cannot place its own window under
//! Wayland (`agent_docs/sticky_notes.md`), so it gets a plain notification with
//! the same text until the GNOME extension learns a card.

#[cfg(not(target_os = "linux"))]
use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, NaiveDate};
#[cfg(not(target_os = "linux"))]
use dioxus::desktop::tao::dpi::LogicalSize;
#[cfg(target_os = "windows")]
use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
use dioxus::desktop::{DesktopContext, WeakDesktopContext};
#[cfg(not(target_os = "linux"))]
use dioxus::desktop::{Config as DesktopConfig, WindowBuilder};
use dioxus::prelude::*;

use super::app::Page;
use crate::config::Config;
use crate::notes::accomplishments::log_for_day;
use crate::notes::day_end::{card_view, is_due, wait_before_next_check, CardView};
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
#[cfg(not(target_os = "linux"))]
use crate::ui::day_end::{DayEndCard, DayEndCardProps, CARD_W, DAY_END_CSS, INITIAL_H, MARGIN, SHADOW};

/// When the config is off, how often to look again.
const IDLE_CHECK: Duration = Duration::from_secs(60);

pub(super) fn setup_day_end_card(
    window: DesktopContext,
    config: Signal<Config>,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    app_ready: Signal<bool>,
    mut current_page: Signal<Page>,
    show_now: Signal<u32>,
) {
    // Bumped by the card's "Open Done" and "N earlier".
    let open_done = use_signal(|| 0u32);
    // The card on screen, if any, so a newer one replaces it. Weak, like the
    // sticky registry: a strong handle would outlive the card's own close and
    // keep its OS window alive.
    let current: Signal<Option<WeakDesktopContext>> = use_signal(|| None);

    use_hook({
        let window = window.clone();
        move || {
            spawn(async move {
                let mut last_shown: Option<NaiveDate> = None;
                // Whether the card was on the last time round, so turning it on
                // (or starting up) after the set time does not show it
                // retroactively. Persisting "shown today" is on the roadmap;
                // this is the simple version, and "Show it now" covers the gap.
                let mut was_on = false;

                loop {
                    let (at, days, fade_secs) = {
                        let c = config.peek();
                        (c.day_end.at(), c.day_end.days.clone(), c.day_end.fade_secs)
                    };
                    let Some(at) = at else {
                        was_on = false;
                        tokio::time::sleep(IDLE_CHECK).await;
                        continue;
                    };

                    let now = Local::now().naive_local();
                    if !was_on {
                        was_on = true;
                        if now.time() >= at {
                            last_shown = Some(now.date());
                        }
                    }

                    // Not before the splash has closed: the card would land on it.
                    if *app_ready.peek() && is_due(now, at, &days, last_shown) {
                        // Latched even when nothing is logged: a day with nothing
                        // at the set time gets no card, rather than one popping up
                        // mid-sentence at 17:40 when the first line lands.
                        last_shown = Some(now.date());
                        show_card(&window, notes, tasks, fade_secs, open_done, current).await;
                    }

                    tokio::time::sleep(wait_before_next_check(Local::now().naive_local(), at)).await;
                }
            });
        }
    });

    // "Show it now" in Settings: today's list, whatever the clock says.
    use_effect({
        let window = window.clone();
        move || {
            if *show_now.read() == 0 {
                return;
            }
            let window = window.clone();
            spawn(async move {
                let fade_secs = config.peek().day_end.fade_secs;
                if !show_card(&window, notes, tasks, fade_secs, open_done, current).await {
                    tracing::info!("Day-end card preview: nothing logged today, so no card");
                }
            });
        }
    });

    // The card asked for the Done page.
    use_effect({
        let window = window.clone();
        move || {
            if *open_done.read() == 0 {
                return;
            }
            current_page.set(Page::Done);
            super::app_menu::show_main_window(&window);
        }
    });
}

/// Build today's card and put it on screen. `false` when nothing was logged
/// today: no entries, no card.
async fn show_card(
    window: &DesktopContext,
    notes: Signal<NoteStore>,
    tasks: Signal<TaskStore>,
    fade_secs: u32,
    open_done: Signal<u32>,
    current: Signal<Option<WeakDesktopContext>>,
) -> bool {
    let now = Local::now();
    let day = now.date_naive();
    let entries = log_for_day(&notes.peek(), &tasks.peek(), day);
    let Some(view) = card_view(&entries, day, now.time()) else {
        return false;
    };
    tracing::info!("Showing the day-end card ({})", view.count);
    open_card(window, view, fade_secs, open_done, current).await;
    true
}

/// Linux: no card window yet, so the text goes out as a notification.
#[cfg(target_os = "linux")]
async fn open_card(
    _window: &DesktopContext,
    view: CardView,
    _fade_secs: u32,
    _open_done: Signal<u32>,
    _current: Signal<Option<WeakDesktopContext>>,
) {
    crate::orchestrator::notify::show_notification(&view.heading, &view.copy_text);
}

/// Windows/macOS: a small transparent always-on-top window, made the way the
/// splash is, except it takes clicks and starts hidden. The card sizes it to
/// itself and tucks it into the corner once it is mounted.
#[cfg(not(target_os = "linux"))]
async fn open_card(
    window: &DesktopContext,
    view: CardView,
    fade_secs: u32,
    open_done: Signal<u32>,
    mut current: Signal<Option<WeakDesktopContext>>,
) {
    let area = super::work_area::primary_work_rect(window);
    let anchor = (
        f64::from(area.x) + f64::from(area.w) - MARGIN,
        f64::from(area.y) + f64::from(area.h) - MARGIN,
    );

    #[allow(unused_mut)]
    let mut builder = WindowBuilder::new()
        .with_title("Beamer day-end")
        .with_decorations(false)
        .with_resizable(false)
        .with_transparent(true)
        .with_always_on_top(true)
        // Never steal the keyboard: it arrives while you are typing somewhere else.
        .with_focusable(false)
        .with_inner_size(LogicalSize::new(CARD_W + SHADOW, INITIAL_H));
    #[cfg(target_os = "windows")]
    let builder = builder.with_skip_taskbar(true).with_undecorated_shadow(false);

    let cfg = DesktopConfig::new()
        .with_data_directory(super::webview_data_dir())
        .with_window(builder)
        .with_background_color((0, 0, 0, 0))
        .with_custom_head(format!(
            "<style>{}{}</style>",
            crate::ui::fonts::embedded_font_css(),
            DAY_END_CSS
        ))
        .with_exits_when_last_window_closes(false);

    let dom = VirtualDom::new_with_props(
        DayEndCard,
        DayEndCardProps { view, anchor, fade_secs, open_done },
    );
    let ctx: DesktopContext = window.new_window(dom, cfg).await;

    // Swap after the await, not before: two showings that overlap (a double-click
    // on "Show it now", or the timer firing mid-preview) both reach this point,
    // and the later one closes the earlier, so a card can never be orphaned. It
    // also replaces a card from an earlier day that was never clicked.
    let previous = current.write().replace(Rc::downgrade(&ctx)).and_then(|weak| weak.upgrade());
    if let Some(old) = previous {
        old.close();
    }
}

//! Inline month calendar for tasks whose due phrase the model couldn't resolve.
//! Replaces the native date popup, which the WebView draws unthemed. It expands
//! in flow below the row: `.task-group` has `overflow: hidden`, so a floating
//! popup would be clipped mid-month.

use chrono::{Datelike, NaiveDate};
use dioxus::prelude::*;

/// Weekday header, Monday-first to match the grid below.
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// "September 2026" for the panel header. Falls back to "" on an impossible
/// month, which the component never passes.
pub fn month_label(year: i32, month: u32) -> String {
    NaiveDate::from_ymd_opt(year, month, 1)
        .map(|first| first.format("%B %Y").to_string())
        .unwrap_or_default()
}

/// The month before, with year rollover.
pub fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month <= 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

/// The month after, with year rollover.
pub fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month >= 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

/// One cell per grid slot, Monday-first, `None` for padding. Always a whole
/// number of weeks, so the caller can lay it out seven across with no math.
pub fn calendar_cells(year: i32, month: u32) -> Vec<Option<NaiveDate>> {
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return Vec::new();
    };
    let (next_year, next_month) = next_month(year, month);
    let Some(first_of_next) = NaiveDate::from_ymd_opt(next_year, next_month, 1) else {
        return Vec::new();
    };
    let days = (first_of_next - first).num_days().max(0) as u32;
    let leading = first.weekday().num_days_from_monday() as usize;
    let mut cells = Vec::with_capacity(leading + days as usize);
    cells.extend(std::iter::repeat_n(None, leading));
    cells.extend((1..=days).filter_map(|d| NaiveDate::from_ymd_opt(year, month, d).map(Some)));
    while cells.len() % 7 != 0 {
        cells.push(None);
    }
    cells
}

/// A picked day as the store expects it: bare `YYYY-MM-DD`, all-day.
pub fn format_day(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

#[derive(Props, Clone, PartialEq)]
pub(super) struct DueCalendarProps {
    pub today: NaiveDate,
    pub on_pick: EventHandler<NaiveDate>,
    pub on_close: EventHandler<()>,
}

/// One month grid plus prev/next and a Today/Close footer. Stateless apart
/// from the visible month, which starts on the month holding `today`.
#[component]
pub(super) fn DueCalendar(props: DueCalendarProps) -> Element {
    let mut visible = use_signal(|| (props.today.year(), props.today.month()));
    let (year, month) = *visible.read();
    let cells = calendar_cells(year, month);

    rsx! {
        div { class: "task-cal",
            div { class: "task-cal-header",
                button {
                    class: "task-cal-nav",
                    title: "Previous month",
                    onclick: move |_| {
                        let (y, m) = *visible.read();
                        visible.set(prev_month(y, m));
                    },
                    "\u{2039}"
                }
                span { class: "task-cal-title", "{month_label(year, month)}" }
                button {
                    class: "task-cal-nav",
                    title: "Next month",
                    onclick: move |_| {
                        let (y, m) = *visible.read();
                        visible.set(next_month(y, m));
                    },
                    "\u{203A}"
                }
            }
            div { class: "task-cal-grid",
                for dow in WEEKDAYS {
                    span { class: "task-cal-dow", "{dow}" }
                }
                for cell in cells {
                    if let Some(day) = cell {
                        button {
                            class: if day == props.today { "task-cal-day today" } else { "task-cal-day" },
                            title: "{format_day(day)}",
                            onclick: move |_| props.on_pick.call(day),
                            "{day.day()}"
                        }
                    } else {
                        span { class: "task-cal-empty" }
                    }
                }
            }
            div { class: "task-cal-footer",
                button {
                    class: "task-cal-action",
                    title: "Set the due date to today",
                    onclick: move |_| props.on_pick.call(props.today),
                    "Today"
                }
                button {
                    class: "task-cal-action",
                    title: "Close without setting a date",
                    onclick: move |_| props.on_close.call(()),
                    "Close"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn the_grid_starts_on_monday() {
        // 1 Sep 2026 is a Tuesday, so one blank leads.
        let cells = calendar_cells(2026, 9);
        assert_eq!(cells.len() % 7, 0, "every week must be whole");
        assert_eq!(cells[0], None);
        assert_eq!(cells[1], Some(ymd(2026, 9, 1)));
    }

    #[test]
    fn month_navigation_rolls_the_year_over() {
        assert_eq!(prev_month(2026, 1), (2025, 12));
        assert_eq!(next_month(2026, 12), (2027, 1));
        assert_eq!(prev_month(2026, 9), (2026, 8));
        assert_eq!(next_month(2026, 9), (2026, 10));
    }
}

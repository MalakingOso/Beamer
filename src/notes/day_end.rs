//! The day-end card's brains, all pure: when it is due, what it says and which
//! lines it shows. The window is `ui::day_end`; the timer that asks is
//! `ui::app_day_end`. The lines are the Done page's own (`log_for_day`).

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

use super::accomplishments::{summary, LogEntry};

/// Lines shown on the card. More than this shows the latest few plus "N earlier".
pub const MAX_LINES: usize = 7;

/// How late the card may still appear. A machine that slept through the set
/// time shows it on wake only inside this window; after that the day is
/// skipped rather than celebrating at 22:00.
pub const GRACE: Duration = Duration::minutes(60);

/// The longest the timer sleeps between looks at the config.
const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether the card should appear now. Local wall-clock arithmetic, never
/// `+ 24h`, so DST days need no special case: a set time that does not exist
/// on a spring-forward day is simply reached when the clock jumps past it.
/// `days` are weekdays, 1 = Monday .. 7 = Sunday.
pub fn is_due(now: NaiveDateTime, at: NaiveTime, days: &[u8], last_shown: Option<NaiveDate>) -> bool {
    let today = now.date();
    if last_shown == Some(today) {
        return false;
    }
    if !days.contains(&(today.weekday().number_from_monday() as u8)) {
        return false;
    }
    let fire = today.and_time(at);
    now >= fire && now - fire <= GRACE
}

/// How long the timer sleeps before looking again: right up to the set time,
/// else at most a minute, so a settings change takes effect within one.
pub fn wait_before_next_check(now: NaiveDateTime, at: NaiveTime) -> std::time::Duration {
    let fire = now.date().and_time(at);
    let until = if now < fire { fire - now } else { Duration::seconds(60) };
    until.to_std().unwrap_or(MAX_WAIT).clamp(std::time::Duration::from_secs(1), MAX_WAIT)
}

/// The card's one-line heading: short, warm, no exclamation marks, never a
/// comparison with another day. Count wins over weekday.
pub fn heading(day: NaiveDate, count: usize) -> &'static str {
    if count == 1 {
        "One thing, done."
    } else if count >= 10 {
        "Big day."
    } else {
        match day.weekday() {
            Weekday::Mon => "Monday, handled.",
            Weekday::Fri => "Friday, done.",
            Weekday::Sat => "Even on a Saturday.",
            Weekday::Sun => "Even on a Sunday.",
            _ => "That's a wrap.",
        }
    }
}

/// One row of the card.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLine {
    pub time: String,
    pub text: String,
}

/// Everything the card draws, decided before any window exists.
#[derive(Debug, Clone, PartialEq)]
pub struct CardView {
    pub heading: String,
    /// "Sunday · 17:30".
    pub caption: String,
    /// "7 things", the small caption on the right, never the headline.
    pub count: String,
    /// Entries before the shown lines (0 when everything fits).
    pub earlier: usize,
    /// The latest `MAX_LINES`, oldest first.
    pub lines: Vec<CardLine>,
    /// The Done page's Copy text for the whole day, not just the shown lines.
    pub copy_text: String,
}

/// The card for a day's entries (oldest first, as `log_for_day` returns them),
/// or `None` for a day with nothing logged: no entries, no card.
pub fn card_view(entries: &[LogEntry], day: NaiveDate, clock: NaiveTime) -> Option<CardView> {
    if entries.is_empty() {
        return None;
    }
    let n = entries.len();
    let earlier = n.saturating_sub(MAX_LINES);
    let lines = entries[earlier..]
        .iter()
        .map(|e| CardLine {
            time: e.when.format("%H:%M").to_string(),
            // A multi-line entry is one row, as in the Done page's Copy text.
            text: e.text.split_whitespace().collect::<Vec<_>>().join(" "),
        })
        .collect();
    let all: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
    Some(CardView {
        heading: heading(day, n).to_string(),
        caption: format!("{} \u{b7} {}", day.format("%A"), clock.format("%H:%M")),
        count: if n == 1 { "1 thing".to_string() } else { format!("{n} things") },
        earlier,
        lines,
        copy_text: summary(day, &all),
    })
}

#[cfg(test)]
#[path = "day_end_tests.rs"]
mod tests;

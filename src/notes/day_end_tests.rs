use super::*;
use chrono::{Local, TimeZone};

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn at(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap()
}

fn on(d: NaiveDate, h: u32, m: u32, s: u32) -> NaiveDateTime {
    d.and_hms_opt(h, m, s).unwrap()
}

const EVERY_DAY: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

// 2026-11-09 is a Monday, so 2026-11-08 is a Sunday.
fn sunday() -> NaiveDate {
    day(2026, 11, 8)
}

fn entry(h: u32, m: u32, text: &str) -> LogEntry {
    LogEntry {
        id: format!("e{h}{m}"),
        when: Local.with_ymd_and_hms(2026, 11, 8, h, m, 0).unwrap(),
        text: text.to_string(),
        is_task: false,
    }
}

// ---- when it is due -------------------------------------------------------

#[test]
fn it_is_not_due_before_the_set_time() {
    assert!(!is_due(on(sunday(), 17, 29, 59), at(17, 30), &EVERY_DAY, None));
}

#[test]
fn it_is_due_from_the_set_time_until_the_grace_runs_out() {
    assert!(is_due(on(sunday(), 17, 30, 0), at(17, 30), &EVERY_DAY, None));
    assert!(is_due(on(sunday(), 18, 30, 0), at(17, 30), &EVERY_DAY, None), "exactly at the grace edge");
    assert!(!is_due(on(sunday(), 18, 30, 1), at(17, 30), &EVERY_DAY, None), "a wake after that skips the day");
}

#[test]
fn it_shows_once_a_day() {
    let now = on(sunday(), 17, 31, 0);
    assert!(!is_due(now, at(17, 30), &EVERY_DAY, Some(sunday())));
    let yesterday = sunday().pred_opt().unwrap();
    assert!(is_due(now, at(17, 30), &EVERY_DAY, Some(yesterday)), "yesterday's showing does not count");
}

#[test]
fn it_respects_the_days_that_are_switched_on() {
    let weekdays = [1, 2, 3, 4, 5];
    let sat = day(2026, 11, 7);
    assert!(!is_due(on(sat, 17, 30, 0), at(17, 30), &weekdays, None));
    assert!(is_due(on(day(2026, 11, 6), 17, 30, 0), at(17, 30), &weekdays, None), "Friday");
    assert!(!is_due(on(sunday(), 17, 30, 0), at(17, 30), &[], None), "no days at all means never");
}

#[test]
fn a_set_time_the_clock_jumps_past_still_fires_inside_the_grace() {
    // The wall clock goes 01:59 -> 03:00 on a spring-forward morning, so a
    // 02:30 set time is never read off the clock. (A naive time cannot model
    // the gap itself; this is the observable behaviour: the first look after
    // the jump is past the set time and inside the grace.)
    let d = day(2026, 3, 8);
    assert!(!is_due(on(d, 1, 59, 0), at(2, 30), &EVERY_DAY, None));
    assert!(is_due(on(d, 3, 0, 0), at(2, 30), &EVERY_DAY, None));
}

#[test]
fn the_timer_sleeps_up_to_the_set_time_but_never_longer_than_a_minute() {
    let secs = |d: std::time::Duration| d.as_secs();
    assert_eq!(secs(wait_before_next_check(on(sunday(), 9, 0, 0), at(17, 30))), 60, "far off: a minute");
    assert_eq!(secs(wait_before_next_check(on(sunday(), 17, 29, 55), at(17, 30))), 5, "close: right up to it");
    assert_eq!(secs(wait_before_next_check(on(sunday(), 17, 30, 0), at(17, 30))), 60, "at it: a minute");
    assert_eq!(secs(wait_before_next_check(on(sunday(), 17, 29, 59), at(17, 30))), 1, "never a busy loop");
    assert_eq!(secs(wait_before_next_check(on(sunday(), 20, 0, 0), at(17, 30))), 60, "after it: a minute");
}

// ---- what it says ---------------------------------------------------------

#[test]
fn headings_follow_the_day() {
    let heading_on = |d, n| heading(day(2026, 11, d), n);
    assert_eq!(heading_on(3, 4), "That's a wrap."); // Tuesday
    assert_eq!(heading_on(9, 4), "Monday, handled.");
    assert_eq!(heading_on(6, 4), "Friday, done.");
    assert_eq!(heading_on(7, 4), "Even on a Saturday.");
    assert_eq!(heading_on(8, 4), "Even on a Sunday.");
}

#[test]
fn the_count_outranks_the_weekday() {
    assert_eq!(heading(day(2026, 11, 9), 1), "One thing, done.", "a Monday with one line");
    assert_eq!(heading(day(2026, 11, 8), 10), "Big day.", "a Sunday with ten");
    assert_eq!(heading(day(2026, 11, 8), 9), "Even on a Sunday.");
}

// ---- what it shows --------------------------------------------------------

#[test]
fn a_day_with_nothing_logged_gets_no_card() {
    assert_eq!(card_view(&[], sunday(), at(17, 30)), None);
}

#[test]
fn a_small_day_shows_every_line_in_order() {
    let entries = [entry(9, 14, "Fixed the sync loop"), entry(10, 40, "Email landlord")];
    let view = card_view(&entries, sunday(), at(17, 30)).unwrap();
    assert_eq!(view.heading, "Even on a Sunday.");
    assert_eq!(view.caption, "Sunday \u{b7} 17:30");
    assert_eq!(view.count, "2 things");
    assert_eq!(view.earlier, 0);
    assert_eq!(
        view.lines,
        vec![
            CardLine { time: "09:14".into(), text: "Fixed the sync loop".into() },
            CardLine { time: "10:40".into(), text: "Email landlord".into() },
        ]
    );
}

#[test]
fn a_single_line_is_singular() {
    let view = card_view(&[entry(9, 14, "Shipped it")], sunday(), at(17, 30)).unwrap();
    assert_eq!(view.count, "1 thing");
    assert_eq!(view.heading, "One thing, done.");
}

#[test]
fn a_big_day_shows_the_latest_seven_and_counts_the_rest() {
    let entries: Vec<LogEntry> = (0..12).map(|i| entry(8 + i, 0, &format!("thing {i}"))).collect();
    let view = card_view(&entries, sunday(), at(17, 30)).unwrap();
    assert_eq!(view.count, "12 things");
    assert_eq!(view.earlier, 5);
    assert_eq!(view.lines.len(), MAX_LINES);
    assert_eq!(view.lines.first().unwrap().text, "thing 5", "the latest seven, oldest of them first");
    assert_eq!(view.lines.last().unwrap().text, "thing 11");
    assert_eq!(view.heading, "Big day.");
}

#[test]
fn exactly_seven_is_not_an_overflow() {
    let entries: Vec<LogEntry> = (0..7).map(|i| entry(8 + i, 0, "x")).collect();
    let view = card_view(&entries, sunday(), at(17, 30)).unwrap();
    assert_eq!(view.earlier, 0);
    assert_eq!(view.lines.len(), 7);
}

#[test]
fn a_multi_line_entry_is_one_row() {
    let view = card_view(&[entry(9, 0, "reviewed\n  the   change")], sunday(), at(17, 30)).unwrap();
    assert_eq!(view.lines[0].text, "reviewed the change");
}

#[test]
fn copy_text_covers_the_whole_day_not_only_the_shown_lines() {
    let entries: Vec<LogEntry> = (0..9).map(|i| entry(8 + i, 0, &format!("thing {i}"))).collect();
    let view = card_view(&entries, sunday(), at(17, 30)).unwrap();
    assert!(view.copy_text.starts_with("Done on Sunday, 8 November 2026"));
    assert!(view.copy_text.contains("- thing 0"), "the earliest line is still in the copy");
    assert!(view.copy_text.contains("- thing 8"));
}

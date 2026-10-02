//! Done page: a log of what got finished, one line per entry, a day at a time.
//! Entries are `NoteKind::Accomplishment` records in `notes::NoteStore` (see
//! `notes::accomplishments`). The dictation hotkey types into the focused
//! field, so dictating an entry here works with no extra hotkey.
//!
//! The viewed day is `None` while following today, so a window left open past
//! midnight rolls over instead of showing yesterday as "Today".

use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use dioxus::prelude::*;

use crate::notes::accomplishments::{log_days, log_for_day, summary, LogEntry};
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::ui::icons::{IconCheck, IconCopy};

#[derive(Props, Clone, PartialEq)]
pub struct DonePageProps {
    pub notes: Signal<NoteStore>,
    /// Only for `flush_stores`, the document's one writer.
    pub tasks: Signal<TaskStore>,
}

fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        "Today".to_string()
    } else if day == today - Duration::days(1) {
        "Yesterday".to_string()
    } else {
        day.format("%a, %-d %b %Y").to_string()
    }
}

fn count_label(n: usize) -> String {
    match n {
        0 => "Nothing logged".to_string(),
        1 => "1 thing done".to_string(),
        n => format!("{n} things done"),
    }
}

/// Time until one second past the next local midnight. Derived from local
/// datetimes, never `+ 24h`, so 23- and 25-hour DST days land on midnight.
fn until_next_midnight<Tz: TimeZone>(now: &DateTime<Tz>) -> std::time::Duration {
    const FLOOR: std::time::Duration = std::time::Duration::from_secs(1);
    let next = (now.date_naive() + Duration::days(1))
        .and_hms_opt(0, 0, 1)
        .expect("00:00:01 is a valid time");
    match now.timezone().from_local_datetime(&next).earliest() {
        Some(t) => t.signed_duration_since(now).to_std().unwrap_or(FLOOR).max(FLOOR),
        // Midnight skipped by a DST jump: check again in an hour.
        None => std::time::Duration::from_secs(3600),
    }
}

#[component]
pub fn DonePage(props: DonePageProps) -> Element {
    let mut notes = props.notes;
    let mut tasks = props.tasks;

    let mut viewing: Signal<Option<NaiveDate>> = use_signal(|| None);
    let mut draft = use_signal(String::new);
    // Two-step delete, like the notes board: which entry awaits confirmation.
    let mut pending_delete: Signal<Option<String>> = use_signal(|| None);
    let mut copied = use_signal(|| false);

    // Bumped at each local midnight; reading it here re-renders the page so
    // `today` is recomputed. A suspend that sleeps through midnight fires late,
    // not never: every loop iteration recomputes from the current time.
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            tokio::time::sleep(until_next_midnight(&Local::now())).await;
            tick += 1;
        }
    });
    let _ = tick.read();

    let today = Local::now().date_naive();
    let day = viewing.read().unwrap_or(today);
    let is_today = day == today;

    let rows: Vec<LogEntry> = log_for_day(&notes.read(), &tasks.read(), day);
    let total = rows.len();

    // The arrows hop between days that have entries, so a quiet weekend is not
    // a run of empty clicks. Forward past the last such day lands on today.
    let (earlier, later) = {
        let days = log_days(&notes.read(), &tasks.read()); // newest first
        let earlier = days.iter().copied().find(|d| *d < day);
        let later = days.iter().rev().copied().find(|d| *d > day && *d < today);
        (earlier, later)
    };

    let mut add = move || {
        let text = draft.read().clone();
        // Always lands on today, so jump back to it from a past day.
        if notes.write().log_accomplishment(&text).is_some() {
            crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
            draft.set(String::new());
            viewing.set(None);
            copied.set(false);
        }
    };

    let summary_text = {
        let lines: Vec<&str> = rows.iter().map(|e| e.text.as_str()).collect();
        summary(day, &lines)
    };

    rsx! {
        div { class: "content",
            div { class: "notes-header",
                h1 { class: "notes-title", "Done" }
                span { class: "notes-count", "{count_label(total)}" }
            }

            p { class: "notes-description",
                "Log what you finished as you go, then look back at the end of the day. Tasks you tick off on the Tasks page show up here too. Turn on Done capture in Settings to log a line by voice."
            }

            div { class: "vocab-add-row",
                input {
                    class: "input vocab-add-input",
                    placeholder: "What did you get done?",
                    value: "{draft}",
                    oninput: move |e: Event<FormData>| draft.set(e.value().to_string()),
                    onkeypress: move |e: Event<KeyboardData>| {
                        if e.key() == Key::Enter {
                            add();
                        }
                    },
                }
                button {
                    class: "btn btn-primary btn-small",
                    disabled: draft.read().trim().is_empty(),
                    onclick: move |_| add(),
                    "Add"
                }
            }

            div { class: "done-day-bar",
                button {
                    class: "note-action-btn",
                    title: "Previous day with entries",
                    disabled: earlier.is_none(),
                    onclick: move |_| {
                        pending_delete.set(None);
                        copied.set(false);
                        if earlier.is_some() {
                            viewing.set(earlier);
                        }
                    },
                    "\u{2039}"
                }
                span { class: "done-day-label", "{day_label(day, today)}" }
                button {
                    class: "note-action-btn",
                    title: "Next day with entries",
                    disabled: is_today,
                    onclick: move |_| {
                        pending_delete.set(None);
                        copied.set(false);
                        viewing.set(later);
                    },
                    "\u{203A}"
                }
                if !is_today {
                    button {
                        class: "note-action-btn",
                        onclick: move |_| {
                            pending_delete.set(None);
                            copied.set(false);
                            viewing.set(None);
                        },
                        "Today"
                    }
                }
                if total > 0 {
                    button {
                        class: "note-action-btn done-copy-btn",
                        title: "Copy this day as a list",
                        onclick: move |_| {
                            let text = summary_text.clone();
                            spawn(async move {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    let _ = clipboard.set_text(&text);
                                }
                                copied.set(true);
                                tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
                                copied.set(false);
                            });
                        },
                        if *copied.read() {
                            IconCheck { size: 13 }
                            "Copied"
                        } else {
                            IconCopy { size: 13 }
                            "Copy"
                        }
                    }
                }
            }

            if rows.is_empty() {
                div { class: "empty-state",
                    span { class: "empty-state-text",
                        if is_today { "Nothing done yet today" } else { "Nothing done this day" }
                    }
                    if is_today {
                        span { class: "empty-state-hint",
                            "Type or dictate a line above each time you finish something."
                        }
                    }
                }
            } else {
                for entry in rows.iter() {
                    {
                        let id = entry.id.clone();
                        let text = entry.text.clone();
                        let is_task = entry.is_task;
                        let stamp = entry.when.format("%H:%M").to_string();
                        let confirming = pending_delete.read().as_deref() == Some(id.as_str());
                        rsx! {
                            div { key: "{id}", class: "note-card done-entry",
                                div {
                                    class: if is_task {
                                        "note-stripe note-stripe-violet"
                                    } else {
                                        "note-stripe note-stripe-teal"
                                    },
                                }
                                div { class: "note-card-main",
                                    div { class: "note-card-body", "{text}" }
                                    div { class: "note-card-meta",
                                        if is_task { "{stamp} \u{b7} task completed" } else { "{stamp}" }
                                    }
                                }
                                div { class: "note-card-actions",
                                    if !is_task {
                                    button {
                                        class: if confirming {
                                            "note-action-btn note-action-danger"
                                        } else {
                                            "note-action-btn"
                                        },
                                        onclick: move |_| {
                                            if !confirming {
                                                pending_delete.set(Some(id.clone()));
                                                return;
                                            }
                                            pending_delete.set(None);
                                            notes.write().delete(&id);
                                            crate::notes::flush_stores(
                                                &mut notes.write(),
                                                &mut tasks.write(),
                                            );
                                        },
                                        if confirming { "Really delete?" } else { "Delete" }
                                    }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, LocalResult, NaiveDateTime, Offset, Utc};

    fn secs(d: std::time::Duration) -> u64 {
        d.as_secs()
    }

    #[test]
    fn normal_day() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let now = tz.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
        assert_eq!(secs(until_next_midnight(&now)), 12 * 3600 + 1);
    }

    #[test]
    fn one_second_before_midnight() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let now = tz.with_ymd_and_hms(2026, 10, 1, 23, 59, 59).unwrap();
        assert_eq!(secs(until_next_midnight(&now)), 2);
    }

    /// UTC+0 until 02:00Z on 8 Mar 2026, then UTC+1: local 02:00-03:00 never
    /// happens, so that day is 23 hours long.
    #[derive(Clone, Copy, Debug)]
    struct SpringForward;

    fn switch() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 8).unwrap().and_hms_opt(2, 0, 0).unwrap()
    }

    impl TimeZone for SpringForward {
        type Offset = FixedOffset;
        fn from_offset(_: &FixedOffset) -> Self {
            SpringForward
        }
        fn offset_from_local_date(&self, d: &NaiveDate) -> LocalResult<FixedOffset> {
            self.offset_from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(&self, l: &NaiveDateTime) -> LocalResult<FixedOffset> {
            let hour = FixedOffset::east_opt(3600).unwrap();
            if *l < switch() {
                LocalResult::Single(Utc.fix())
            } else if *l >= switch() + Duration::hours(1) {
                LocalResult::Single(hour)
            } else {
                LocalResult::None
            }
        }
        fn offset_from_utc_date(&self, d: &NaiveDate) -> FixedOffset {
            self.offset_from_utc_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, u: &NaiveDateTime) -> FixedOffset {
            if *u < switch() { Utc.fix() } else { FixedOffset::east_opt(3600).unwrap() }
        }
    }

    #[test]
    fn dst_short_day_is_not_24_hours() {
        let now = SpringForward.with_ymd_and_hms(2026, 3, 8, 0, 30, 0).unwrap();
        // 00:30 -> 00:00:01 next day is 23h30m of wall clock, 22h30m elapsed.
        assert_eq!(secs(until_next_midnight(&now)), 22 * 3600 + 30 * 60 + 1);
    }
}

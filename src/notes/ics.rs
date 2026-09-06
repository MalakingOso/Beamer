//! One accepted task into a calendar entry. Hand-written RFC 5545, one-way:
//! the file lands in the temp dir for the user's calendar app, and Beamer
//! cannot edit or remove it afterwards.
//! ⚠️ Timed values are emitted **floating** (no offset, no `Z`): an offset
//! suffix like `20260825T090000+01:00` is malformed RFC 5545. Only `DTSTAMP` is UTC.

use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use std::path::PathBuf;

use super::task::{Due, Task, TaskKind};

/// Longest content line the spec permits, in octets excluding CRLF.
const FOLD_LIMIT: usize = 75;

/// Escape a TEXT value (RFC 5545 §3.3.11). Backslash first, or the others'
/// escapes get double-escaped.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {} // bare CR carries no meaning here; dropping keeps the line intact
            _ => out.push(ch),
        }
    }
    out
}

/// Fold a content line to 75 octets, continuing with CRLF + one space.
/// Counts octets but breaks on character boundaries: a byte-wise split would
/// panic on or corrupt multi-byte text.
fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / FOLD_LIMIT * 3);
    let mut octets = 0usize;
    for ch in line.chars() {
        let width = ch.len_utf8();
        if octets + width > FOLD_LIMIT {
            out.push_str("\r\n ");
            // The leading space counts toward the continuation line's length.
            octets = 1;
        }
        out.push(ch);
        octets += width;
    }
    out
}

fn date(d: NaiveDate) -> String {
    d.format("%Y%m%d").to_string()
}

/// Floating local datetime: no offset, no `Z`.
fn date_time(dt: NaiveDateTime) -> String {
    dt.format("%Y%m%dT%H%M%S").to_string()
}

/// How long an event with no stated end runs for.
const DEFAULT_EVENT_HOURS: i64 = 1;

/// Build a complete `.ics` for one task. `now` is a parameter so tests are
/// reproducible. An undated task still exports as a `VTODO` with no `DUE`.
pub fn calendar(task: &Task, now: DateTime<Utc>) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Beamer//Notes//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "METHOD:PUBLISH".to_string(), // a file for the user's own calendar, not an invitation
    ];

    let due = task.due_parsed();
    let component = match task.kind {
        // A dateless "event" has nothing to start at; degrade to VTODO, not an invalid VEVENT.
        TaskKind::Event if due.is_some() => "VEVENT",
        _ => "VTODO",
    };

    lines.push(format!("BEGIN:{component}"));
    // Stable UID so re-exporting updates the calendar's copy instead of duplicating it.
    lines.push(format!("UID:beamer-{}@beamer.app", task.id));
    lines.push(format!("DTSTAMP:{}Z", now.naive_utc().format("%Y%m%dT%H%M%S")));
    lines.push(format!("SUMMARY:{}", escape(&task.text)));

    if !task.evidence.trim().is_empty() {
        lines.push(format!("DESCRIPTION:{}", escape(&task.evidence)));
    }

    match (component, due) {
        ("VEVENT", Some(Due::At(start))) => {
            lines.push(format!("DTSTART:{}", date_time(start)));
            lines.push(format!(
                "DTEND:{}",
                date_time(start + Duration::hours(DEFAULT_EVENT_HOURS))
            ));
        }
        ("VEVENT", Some(Due::AllDay(day))) => {
            lines.push(format!("DTSTART;VALUE=DATE:{}", date(day)));
            // DTEND is exclusive: same-date would be a zero-length event most calendars hide.
            lines.push(format!("DTEND;VALUE=DATE:{}", date(day + Duration::days(1))));
        }
        (_, Some(Due::At(at))) => lines.push(format!("DUE:{}", date_time(at))),
        (_, Some(Due::AllDay(day))) => {
            lines.push(format!("DUE;VALUE=DATE:{}", date(day)));
        }
        (_, None) => {}
    }

    if matches!(component, "VTODO") && task.done {
        lines.push("STATUS:COMPLETED".to_string());
        lines.push("PERCENT-COMPLETE:100".to_string());
    }

    lines.push(format!("END:{component}"));
    lines.push("END:VCALENDAR".to_string());

    let mut out: String =
        lines.iter().map(|l| format!("{}\r\n", fold(l))).collect();
    // Some parsers are unhappy with a file that does not end in a line break.
    if !out.ends_with("\r\n") {
        out.push_str("\r\n");
    }
    out
}

/// A file name from task text plus the task id, restricted to shell- and
/// filesystem-safe characters. The id suffix keeps two tasks that start with
/// the same words from overwriting each other in the temp dir.
pub fn file_name(task: &Task) -> String {
    let slug: String = task
        .text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    // Task ids are hex/timestamp shapes already, but sanitize defensively:
    // the name must stay filesystem-safe whatever mints the id.
    let id_suffix: String = task
        .id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .filter(|c| *c != '-')
        .take(24)
        .collect();
    if slug.is_empty() {
        format!("beamer-task-{id_suffix}.ics")
    } else {
        format!("{slug}-{id_suffix}.ics")
    }
}

/// Write the task's `.ics` to the temp dir and return the path.
pub fn write_temp(task: &Task) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(file_name(task));
    std::fs::write(&path, calendar(task, Utc::now()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::task::TaskStatus;

    fn stamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-23T11:30:00Z").unwrap().with_timezone(&Utc)
    }

    fn task(text: &str, due: Option<&str>, all_day: bool, kind: TaskKind) -> Task {
        Task {
            id: "18f2-0001".into(),
            note_id: "note-1".into(),
            text: text.into(),
            evidence: "I need to ring Sarah".into(),
            confidence: 0.9,
            status: TaskStatus::Accepted,
            done: false,
            created: "2026-08-23T10:00:00+01:00".into(),
            decided: Some("2026-08-23T10:01:00+01:00".into()),
            due: due.map(str::to_string),
            due_all_day: all_day,
            due_phrase: Some("before Friday".into()),
            kind,
        }
    }

    /// Unfold back into logical lines, which is what assertions should read.
    fn unfolded(ics: &str) -> Vec<String> {
        ics.replace("\r\n ", "").lines().map(str::to_string).collect()
    }

    #[test]
    fn a_todo_emits_a_vtodo_with_a_due_date() {
        let ics = calendar(&task("Ring Sarah", Some("2026-08-28"), true, TaskKind::Todo), stamp());
        let lines = unfolded(&ics);

        assert!(lines.contains(&"BEGIN:VTODO".to_string()));
        assert!(lines.contains(&"DUE;VALUE=DATE:20260828".to_string()));
        assert!(lines.contains(&"SUMMARY:Ring Sarah".to_string()));
        assert!(lines.contains(&"DTSTAMP:20260823T113000Z".to_string()));
        assert!(lines.contains(&"UID:beamer-18f2-0001@beamer.app".to_string()));
        assert!(!ics.contains("VEVENT"));
    }

    #[test]
    fn an_event_emits_a_vevent_with_a_start_and_an_end() {
        let ics = calendar(
            &task("Standup", Some("2026-08-25T09:00:00"), false, TaskKind::Event),
            stamp(),
        );
        let lines = unfolded(&ics);

        assert!(lines.contains(&"BEGIN:VEVENT".to_string()));
        assert!(lines.contains(&"DTSTART:20260825T090000".to_string()));
        assert!(
            lines.contains(&"DTEND:20260825T100000".to_string()),
            "an event with no stated end runs an hour"
        );
        assert!(!ics.contains("VTODO"));
    }

    #[test]
    fn a_timed_value_carries_no_offset_and_no_z() {
        // An offset suffix is not a valid DATE-TIME, however plausible it looks.
        let ics = calendar(
            &task("Standup", Some("2026-08-25T09:00:00"), false, TaskKind::Event),
            stamp(),
        );
        for line in unfolded(&ics) {
            let Some((name, value)) = line.split_once(':') else { continue };
            if !matches!(name, "DTSTART" | "DTEND" | "DUE") {
                continue;
            }
            assert!(
                !value.contains('+') && !value.contains('Z') && !value[1..].contains('-'),
                "a UTC-offset suffix is not a valid DATE-TIME: {line}"
            );
        }
        assert!(ics.contains("DTSTAMP:20260823T113000Z"), "DTSTAMP alone must be UTC");
    }

    #[test]
    fn an_all_day_event_ends_on_the_following_date() {
        // DTEND is exclusive; same-date is a zero-length event.
        let ics = calendar(&task("Off", Some("2026-08-25"), true, TaskKind::Event), stamp());
        let lines = unfolded(&ics);
        assert!(lines.contains(&"DTSTART;VALUE=DATE:20260825".to_string()));
        assert!(lines.contains(&"DTEND;VALUE=DATE:20260826".to_string()));
    }

    #[test]
    fn an_event_with_no_date_degrades_to_a_todo_rather_than_emitting_an_invalid_vevent() {
        let ics = calendar(&task("Standup", None, false, TaskKind::Event), stamp());
        assert!(ics.contains("BEGIN:VTODO"));
        assert!(!ics.contains("VEVENT"), "a VEVENT with no DTSTART is not valid");
    }

    #[test]
    fn an_undated_task_still_exports() {
        let ics = calendar(&task("Ring Sarah", None, false, TaskKind::Todo), stamp());
        assert!(ics.contains("BEGIN:VTODO"));
        assert!(!ics.contains("DUE"), "no date is no DUE, not an empty one");
    }

    #[test]
    fn a_summary_with_separators_in_it_is_escaped() {
        // An unescaped comma starts a second value and malforms the file.
        let ics = calendar(
            &task("Ring Sarah, then Bob; re: C:\\deck\nand the notes", None, false, TaskKind::Todo),
            stamp(),
        );
        let summary = unfolded(&ics)
            .into_iter()
            .find(|l| l.starts_with("SUMMARY:"))
            .expect("a summary line");
        assert_eq!(
            summary,
            "SUMMARY:Ring Sarah\\, then Bob\\; re: C:\\\\deck\\nand the notes"
        );
        assert_eq!(
            unfolded(&ics).iter().filter(|l| l.starts_with("SUMMARY")).count(),
            1,
            "an escaped newline must stay on one logical line"
        );
    }

    #[test]
    fn a_backslash_is_escaped_before_the_characters_that_introduce_backslashes() {
        let ics = calendar(&task("a\\b,c", None, false, TaskKind::Todo), stamp());
        assert!(ics.contains("SUMMARY:a\\\\b\\,c"), "{ics}");
    }

    #[test]
    fn a_long_line_folds_at_seventy_five_octets() {
        let ics = calendar(&task(&"word ".repeat(40), None, false, TaskKind::Todo), stamp());
        for line in ics.split("\r\n") {
            assert!(line.len() <= FOLD_LIMIT, "{} octets: {line:?}", line.len());
        }
        assert!(ics.contains("\r\n "), "a 200-character summary must actually fold");
    }

    #[test]
    fn folding_never_splits_a_multi_byte_character() {
        // A byte-wise fold would emit half a codepoint and break UTF-8.
        for text in [
            format!("{} \u{2014} thanks", "Please send the quarterly numbers to accounting"),
            "\u{2014}".repeat(90),
            "你好世界".repeat(30),
        ] {
            let ics = calendar(&task(&text, None, false, TaskKind::Todo), stamp());
            let summary = unfolded(&ics)
                .into_iter()
                .find(|l| l.starts_with("SUMMARY:"))
                .expect("a summary line");
            assert_eq!(summary, format!("SUMMARY:{}", escape(&text)));
            for line in ics.split("\r\n") {
                assert!(line.len() <= FOLD_LIMIT, "{} octets on {text:?}", line.len());
            }
        }
    }

    #[test]
    fn every_line_ends_crlf_and_the_file_does_too() {
        let ics = calendar(&task("Ring Sarah", Some("2026-08-28"), true, TaskKind::Todo), stamp());
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        assert!(!ics.contains('\n') || ics.matches('\n').count() == ics.matches("\r\n").count());
    }

    #[test]
    fn a_completed_todo_says_so() {
        let mut t = task("Ring Sarah", Some("2026-08-28"), true, TaskKind::Todo);
        t.done = true;
        let ics = calendar(&t, stamp());
        assert!(ics.contains("STATUS:COMPLETED"));
        assert!(ics.contains("PERCENT-COMPLETE:100"));
    }

    #[test]
    fn the_uid_is_stable_so_a_re_export_updates_rather_than_duplicates() {
        let t = task("Ring Sarah", Some("2026-08-28"), true, TaskKind::Todo);
        let first = calendar(&t, stamp());
        let later = calendar(&t, stamp() + Duration::hours(3));
        let uid = |s: &str| {
            unfolded(s).into_iter().find(|l| l.starts_with("UID:")).unwrap()
        };
        assert_eq!(uid(&first), uid(&later));
        assert_ne!(first, later, "only the stamp moves");
    }

    #[test]
    fn the_file_name_survives_arbitrary_task_text() {
        let t = task("Ring Sarah / Bob \"today\"\n", None, false, TaskKind::Todo);
        let name = file_name(&t);
        assert_eq!(name, "ring-sarah-bob-today-18f20001.ics");
        assert!(!name.contains('/') && !name.contains('"') && !name.contains('\n'));

        let blank = task("\u{2014}\u{2014}", None, false, TaskKind::Todo);
        assert_eq!(file_name(&blank), "beamer-task-18f20001.ics");
    }

    #[test]
    fn same_text_tasks_get_distinct_file_names() {
        let mut a = task("Ring Sarah", None, false, TaskKind::Todo);
        let mut b = task("Ring Sarah", None, false, TaskKind::Todo);
        a.id = "18f2-0001-aaaa".into();
        b.id = "18f2-0002-aaaa".into();
        assert_ne!(
            file_name(&a),
            file_name(&b),
            "same words, different tasks — one export must not overwrite the other"
        );
    }
}

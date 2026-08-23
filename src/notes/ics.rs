//! Turning one accepted task into a calendar entry.
//!
//! Hand-written RFC 5545. No new dependency: the subset needed is one `VTODO`
//! or one `VEVENT`, and the parts that are actually easy to get wrong — folding
//! and escaping — are the parts a crate would not save us from testing anyway.
//!
//! **One-way, by design.** The file is written to the temp dir and handed to
//! whatever owns `text/calendar` (GNOME Calendar, Evolution, Thunderbird).
//! Beamer cannot later edit or remove what the calendar imported; that is the
//! accepted consequence of not integrating with Evolution Data Server. EDS over
//! the existing `zbus` dependency is the documented upgrade path if live
//! two-way entries are ever wanted.
//!
//! Nothing here reaches the calendar on its own. Export is a per-task button,
//! which follows directly from the standing decision that tasks are suggestions
//! and nothing is ever added unconfirmed.
//!
//! ⚠️ **Three shapes of DATE-TIME exist and only three**: floating local
//! (`20260825T090000`), UTC (`20260825T090000Z`), and `TZID=`-qualified, which
//! requires a whole `VTIMEZONE` block. An offset suffix — `20260825T090000+01:00`
//! — is **not** one of them and is malformed however plausible it looks. Timed
//! values here are emitted **floating**: "standup at nine" means nine where you
//! are, which is exactly what floating means, and it needs no VTIMEZONE.

use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use std::path::PathBuf;

use super::task::{Due, Task, TaskKind};

/// Longest line the spec permits, in **octets**, excluding the CRLF.
const FOLD_LIMIT: usize = 75;

/// Escape a TEXT value: RFC 5545 §3.3.11.
///
/// Backslash first — escaping it after the others would double-escape the
/// backslashes they just introduced. A note saying "Ring Sarah, then Bob"
/// produces a malformed file without this, because an unescaped comma starts a
/// second value.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            // A bare CR would end the line mid-value. Dropped rather than
            // escaped: it carries no meaning of its own here.
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Fold a content line to 75 octets, continuing with CRLF + one space.
///
/// ⚠️ Counts octets but breaks on **character** boundaries. Splitting at
/// `line[..75]` panics on a multi-byte sequence and, worse, would emit half a
/// codepoint if done over bytes — the same trap `ui::components::truncate_chars`
/// documents, arriving here through task text that routinely carries the smart
/// punctuation the STT backends emit.
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

/// Floating local — no offset, no `Z`. See the module warning.
fn date_time(dt: NaiveDateTime) -> String {
    dt.format("%Y%m%dT%H%M%S").to_string()
}

/// How long an event with no stated end runs for.
const DEFAULT_EVENT_HOURS: i64 = 1;

/// Build a complete `.ics` for one task.
///
/// `now` is a parameter rather than a clock read so the output is reproducible
/// in a test. `DTSTAMP` is the one value that genuinely must be UTC.
///
/// An undated task still exports: a `VTODO` with no `DUE` is valid and lands in
/// the calendar's task list undated, which is more useful than refusing.
pub fn calendar(task: &Task, now: DateTime<Utc>) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Beamer//Notes//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        // PUBLISH, not REQUEST: this is a file handed to the user's own
        // calendar, not an invitation sent to anybody.
        "METHOD:PUBLISH".to_string(),
    ];

    let due = task.due_parsed();
    let component = match task.kind {
        // An event needs a start. A task the model called an appointment but
        // gave no date to has nothing to start at, so it degrades to a VTODO
        // rather than emitting an invalid VEVENT.
        TaskKind::Event if due.is_some() => "VEVENT",
        _ => "VTODO",
    };

    lines.push(format!("BEGIN:{component}"));
    // Stable, so re-exporting the same task updates the calendar's copy rather
    // than adding a duplicate. Prefixed because a bare `next_id` value is not
    // distinctive enough to be globally unique, which UID is required to be.
    lines.push(format!("UID:beamer-{}@beamer.app", task.id));
    lines.push(format!("DTSTAMP:{}Z", now.naive_utc().format("%Y%m%dT%H%M%S")));
    lines.push(format!("SUMMARY:{}", escape(&task.text)));

    if !task.evidence.trim().is_empty() {
        // The span of the note that produced this task. Carried through so the
        // entry can still be checked against what was actually said, which is
        // the whole reason `evidence` exists.
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
            // DTEND is **exclusive** for an all-day event: a one-day event ends
            // on the following date. Emitting the same date gives a zero-length
            // event that several calendars simply do not draw.
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

/// A file name that says what it is when it lands in the temp dir.
///
/// Restricted to characters no shell or filesystem will argue about — task text
/// is arbitrary user prose and can hold slashes, quotes and newlines.
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
    if slug.is_empty() {
        format!("beamer-task-{}.ics", task.id)
    } else {
        format!("{slug}.ics")
    }
}

/// Write the task's `.ics` to the temp dir and return the path.
///
/// Separated from [`calendar`] so everything with a right answer stays pure and
/// tested, and only the file write is untested. The caller opens the result —
/// see `ui::open_external`.
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

    /// Unfold a rendered calendar back into logical lines, which is what a
    /// parser sees and therefore what the assertions should read.
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
        // The trap this test exists for: RFC 5545 DATE-TIME is floating, UTC
        // with Z, or TZID= with a VTIMEZONE block. `20260825T090000+01:00` is
        // none of those and is malformed however plausible it looks.
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
        // DTEND is exclusive. Emitting the same date gives a zero-length event
        // that several calendars do not draw at all.
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
        // "Ring Sarah, then Bob" produces a malformed file without this: an
        // unescaped comma starts a second value.
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
        // Escaping `\` last would double-escape the ones `,` and `;` just added.
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
        // The same failure `truncate_chars` was fixed for, arriving here
        // through the smart punctuation the STT backends emit. A byte-wise fold
        // would emit half a codepoint and the file would not even be UTF-8.
        for text in [
            format!("{} \u{2014} thanks", "Please send the quarterly numbers to accounting"),
            "\u{2014}".repeat(90),
            "你好世界".repeat(30),
        ] {
            let ics = calendar(&task(&text, None, false, TaskKind::Todo), stamp());
            // Reconstructing the value proves nothing was lost or corrupted.
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
        assert_eq!(name, "ring-sarah-bob-today.ics");
        assert!(!name.contains('/') && !name.contains('"') && !name.contains('\n'));

        let blank = task("\u{2014}\u{2014}", None, false, TaskKind::Todo);
        assert_eq!(file_name(&blank), "beamer-task-18f2-0001.ics");
    }
}

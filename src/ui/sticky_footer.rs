//! Note footer: pure function of the extraction field. Keyed to the pass, not
//! the note's origin, so a failed pass stays retryable. Normally a single
//! icon + tooltip; words appear only on the failure path.

use crate::notes::StageState;

/// Which glyph the footer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterIcon {
    /// Outstanding work, or a re-run.
    Asterisk,
    /// A pass for this note is in flight right now.
    Running,
    /// The pass has had its turn.
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
    pub icon: FooterIcon,
    /// Shown on hover; carries the message in the normal case.
    pub tooltip: &'static str,
    /// Short danger-red label; set only on failure.
    pub error: Option<&'static str>,
}

/// Decide the footer from the extraction field and whether a pass for this
/// note is in flight right now. In flight outranks everything else — even a
/// stale `Failed` from a prior attempt reads as "working on it" while a retry
/// is actually running.
pub fn footer(extract: StageState, in_flight: bool) -> Footer {
    use StageState::{Failed, Pending};

    if in_flight {
        return Footer {
            icon: FooterIcon::Running,
            tooltip: "Working on it\u{2026}",
            error: None,
        };
    }
    if extract == Failed {
        return Footer {
            icon: FooterIcon::Asterisk,
            tooltip: "Couldn't look for tasks — press to try again",
            error: Some("Task search failed"),
        };
    }
    if extract == Pending {
        return Footer {
            icon: FooterIcon::Asterisk,
            tooltip: "Find tasks",
            error: None,
        };
    }
    // Done or Skipped (user turned it off) — nothing to nag about.
    Footer {
        icon: FooterIcon::Check,
        tooltip: "Run again",
        error: None,
    }
}

/// Whether the footer's red failure text should (re)start its temporary
/// display. Two cases, and only these two:
///
/// - first mount with the note already failed (a restart, or a window opened
///   after the pass finished): show it once, then let it go quiet;
/// - a pass for this note just finished failed: flash it again.
///
/// Anything else leaves visibility alone: a pass still running never shows
/// words (the Running icon outranks a stale failure), a fresh success shows
/// nothing, and a note that stays failed across unrelated re-renders must not
/// restart its own timer.
pub fn should_flash_error(
    is_mount: bool,
    just_finished: bool,
    running: bool,
    failed: bool,
) -> bool {
    failed && !running && (is_mount || just_finished)
}

#[cfg(test)]
mod tests {
    use super::*;
    use StageState::{Done, Failed, Pending, Skipped};

    #[test]
    fn an_untouched_note_offers_a_pass_without_words() {
        let f = footer(Pending, false);
        assert_eq!(f.icon, FooterIcon::Asterisk);
        assert_eq!(f.tooltip, "Find tasks");
        assert!(
            f.error.is_none(),
            "the normal path is an icon and a tooltip; words are for failures"
        );
    }

    #[test]
    fn a_failed_pass_is_the_one_place_the_footer_uses_words() {
        let f = footer(Failed, false);
        assert_eq!(f.error, Some("Task search failed"));
        assert_eq!(f.icon, FooterIcon::Asterisk);
    }

    #[test]
    fn a_finished_note_is_quiet_but_not_inert() {
        let f = footer(Done, false);
        assert_eq!(f.icon, FooterIcon::Check);
        assert!(f.error.is_none());
        assert_eq!(f.tooltip, "Run again");
    }

    #[test]
    fn a_skipped_pass_offers_nothing_to_retry() {
        assert_eq!(footer(Skipped, false).icon, FooterIcon::Check);
    }

    #[test]
    fn a_note_opened_already_failed_flashes_once_then_goes_quiet() {
        assert!(
            should_flash_error(true, false, false, true),
            "mount with a persisted failure is the only time the words appear unprompted"
        );
    }

    #[test]
    fn a_note_opened_while_its_pass_runs_does_not_flash() {
        assert!(
            !should_flash_error(true, false, true, true),
            "Running outranks even a stale failure, on mount as everywhere else"
        );
    }

    #[test]
    fn a_just_finished_failed_pass_flashes_again() {
        assert!(should_flash_error(false, true, false, true));
        assert!(
            !should_flash_error(false, false, false, true),
            "a note that stays failed across unrelated re-renders must not restart its own timer"
        );
    }

    #[test]
    fn success_or_a_running_pass_never_flashes() {
        assert!(!should_flash_error(true, false, false, false));
        assert!(!should_flash_error(false, true, false, false));
        assert!(!should_flash_error(true, false, true, false));
        assert!(!should_flash_error(false, false, true, true));
    }

    #[test]
    fn in_flight_outranks_even_a_stale_failed_state() {
        let f = footer(Failed, true);
        assert_eq!(f.icon, FooterIcon::Running);
        assert!(
            f.error.is_none(),
            "a retry actually running must not still show the previous attempt's failure text"
        );
    }
}

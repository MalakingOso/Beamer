//! What the note footer offers, given how far the model passes have got.
//!
//! Pure: a function of the two stage fields and nothing else. Split out
//! because this is the part that can be wrong — the rendering is a button and
//! a tooltip.
//!
//! **Keyed to the stage fields, not to the note's origin.** Keying it to
//! "dictated notes have already had their pass" leaves a dead end: a dictated
//! note whose cleanup was superseded by an edit has spent its automatic
//! trigger and would have no way back. Reading the stages instead means the
//! affordance is available exactly when there is something to run.
//!
//! The words are deliberately scarce. The cleanup pass is *ambient* — it has
//! already happened by the time you look at the note — so the normal state is
//! a single icon with a tooltip. A visible label appears only on the failure
//! path, which is the one case where the user genuinely needs telling.

use crate::notes::pipeline::Stages;
use crate::notes::StageState;

/// Which glyph the footer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterIcon {
    /// Something is outstanding, or can be run again.
    Asterisk,
    /// Both passes have had their turn. Quiet by design.
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
    pub icon: FooterIcon,
    /// Shown on hover. Carries the whole message in the normal case.
    pub tooltip: &'static str,
    /// A short label rendered in danger red, and the only case where the
    /// footer uses words at all.
    pub error: Option<&'static str>,
    /// What pressing the footer asks the pipeline for.
    pub stages: Stages,
}

/// Decide the footer from the two stage fields.
///
/// A failure outranks outstanding work: if cleanup failed, that is what the
/// user needs to know, even though extraction may also be waiting.
pub fn footer(clean: StageState, extract: StageState) -> Footer {
    use StageState::{Failed, Pending};

    if clean == Failed {
        // Retrying asks for both, not just cleanup. A failed cleanup is meant
        // to leave extraction to run against `raw`, so the two are one gesture.
        return Footer {
            icon: FooterIcon::Asterisk,
            tooltip: "Cleanup failed — press to try again",
            error: Some("Cleanup failed"),
            stages: Stages::Both,
        };
    }
    if extract == Failed {
        return Footer {
            icon: FooterIcon::Asterisk,
            tooltip: "Couldn't look for tasks — press to try again",
            error: Some("Task search failed"),
            stages: Stages::ExtractOnly,
        };
    }
    match (clean == Pending, extract == Pending) {
        (true, true) => Footer {
            icon: FooterIcon::Asterisk,
            tooltip: "Clean up and find tasks",
            error: None,
            stages: Stages::Both,
        },
        (true, false) => Footer {
            icon: FooterIcon::Asterisk,
            tooltip: "Clean up",
            error: None,
            stages: Stages::CleanOnly,
        },
        (false, true) => Footer {
            icon: FooterIcon::Asterisk,
            tooltip: "Find tasks",
            error: None,
            stages: Stages::ExtractOnly,
        },
        // Everything has had its turn — Done, or Skipped because the user
        // turned it off. Neither is something to nag about.
        (false, false) => Footer {
            icon: FooterIcon::Check,
            tooltip: "Run again",
            error: None,
            stages: Stages::Both,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use StageState::{Done, Failed, Pending, Skipped};

    #[test]
    fn an_untouched_note_offers_both_passes_without_words() {
        let f = footer(Pending, Pending);
        assert_eq!(f.icon, FooterIcon::Asterisk);
        assert_eq!(f.stages, Stages::Both);
        assert!(
            f.error.is_none(),
            "the normal path is an icon and a tooltip; words are for failures"
        );
    }

    #[test]
    fn a_cleaned_note_that_was_never_analysed_offers_only_extraction() {
        let f = footer(Done, Pending);
        assert_eq!(f.stages, Stages::ExtractOnly);
        assert_eq!(f.tooltip, "Find tasks");
    }

    #[test]
    fn a_failed_cleanup_is_the_one_place_the_footer_uses_words() {
        let f = footer(Failed, Pending);
        assert_eq!(f.error, Some("Cleanup failed"));
        assert_eq!(
            f.stages,
            Stages::Both,
            "a failed cleanup leaves extraction to run against raw, so the retry is one gesture"
        );
    }

    #[test]
    fn a_cleanup_failure_outranks_an_extraction_failure() {
        // Both failed means the server was down for both. Naming cleanup is
        // more useful: it is the pass whose result the user can see.
        assert_eq!(footer(Failed, Failed).error, Some("Cleanup failed"));
    }

    #[test]
    fn a_superseded_cleanup_can_still_be_retried() {
        // The dead end this whole module exists to avoid. `apply_cleanup`
        // leaves a superseded pass at Pending precisely so this stays true —
        // an origin-keyed footer would have spent the note's one trigger.
        let f = footer(Pending, Done);
        assert_eq!(f.icon, FooterIcon::Asterisk);
        assert_eq!(f.stages, Stages::CleanOnly);
    }

    #[test]
    fn a_finished_note_is_quiet_but_not_inert() {
        let f = footer(Done, Done);
        assert_eq!(f.icon, FooterIcon::Check);
        assert!(f.error.is_none());
        assert_eq!(f.tooltip, "Run again");
    }

    #[test]
    fn a_skipped_stage_offers_nothing_to_retry() {
        // Skipped means the user turned the feature off. Showing an
        // outstanding-work affordance for it would be nagging about a
        // decision they already made.
        assert_eq!(footer(Skipped, Skipped).icon, FooterIcon::Check);
        assert_eq!(footer(Skipped, Done).icon, FooterIcon::Check);
    }
}

//! `[day_end]`: the card that lists what you finished, at a set time each day.
//! Off by default. The card itself lives in `ui::day_end`; whether it is due,
//! and what it says, in `notes::day_end`.

use chrono::NaiveTime;
use serde::{Deserialize, Serialize};

/// What turning the card on starts with.
pub const DEFAULT_TIME: &str = "17:30";
/// What the "fade away" switch sets; `0` means the card stays until clicked.
pub const FADE_SECS: u32 = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayEndConfig {
    /// Master switch. Kept apart from `time` so turning the card off does not
    /// forget the time it was set to.
    #[serde(default)]
    pub enabled: bool,
    /// Local time as `HH:MM`.
    #[serde(default = "default_time")]
    pub time: String,
    /// Weekdays the card may appear: 1 = Monday .. 7 = Sunday. All on by
    /// default: a day with nothing logged gets no card anyway.
    #[serde(default = "all_days")]
    pub days: Vec<u8>,
    /// Seconds before the card leaves by itself; `0` = stays until clicked.
    #[serde(default)]
    pub fade_secs: u32,
}

fn default_time() -> String {
    DEFAULT_TIME.into()
}

fn all_days() -> Vec<u8> {
    (1..=7).collect()
}

impl Default for DayEndConfig {
    fn default() -> Self {
        Self { enabled: false, time: default_time(), days: all_days(), fade_secs: 0 }
    }
}

impl DayEndConfig {
    /// The time to fire, or `None` when the card is off or the time is unreadable.
    pub fn at(&self) -> Option<NaiveTime> {
        if !self.enabled {
            return None;
        }
        parse_time(&self.time)
    }
}

/// `HH:MM` (what `<input type="time">` yields), tolerating `HH:MM:SS`.
pub fn parse_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    NaiveTime::parse_from_str(s, "%H:%M").or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_is_off_by_default_and_remembers_its_time() {
        let cfg = DayEndConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.at(), None, "off means no time to fire");
        assert_eq!(cfg.time, "17:30");
        assert_eq!(cfg.days, vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(cfg.fade_secs, 0, "stays until clicked unless asked otherwise");
    }

    #[test]
    fn an_enabled_card_reports_its_time() {
        let cfg = DayEndConfig { enabled: true, ..Default::default() };
        assert_eq!(cfg.at(), NaiveTime::from_hms_opt(17, 30, 0));
    }

    #[test]
    fn an_unreadable_time_is_off_rather_than_midnight() {
        let cfg = DayEndConfig { enabled: true, time: "teatime".into(), ..Default::default() };
        assert_eq!(cfg.at(), None);
    }

    #[test]
    fn time_parsing_accepts_what_a_time_input_sends() {
        assert_eq!(parse_time("09:05"), NaiveTime::from_hms_opt(9, 5, 0));
        assert_eq!(parse_time(" 17:30:15 "), NaiveTime::from_hms_opt(17, 30, 15));
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("25:00"), None);
    }

    #[test]
    fn an_old_config_without_the_section_loads_with_defaults() {
        let cfg: crate::config::Config = toml::from_str("[recording]\nhotkey = \"Ctrl+Space\"\n").unwrap();
        assert_eq!(cfg.day_end, DayEndConfig::default());
    }

    #[test]
    fn a_partial_section_fills_in_the_rest() {
        let cfg: crate::config::Config = toml::from_str("[day_end]\nenabled = true\n").unwrap();
        assert!(cfg.day_end.enabled);
        assert_eq!(cfg.day_end.time, "17:30");
        assert_eq!(cfg.day_end.days.len(), 7);
    }
}

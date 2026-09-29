//! Snooze's choices, as Gmail offers them: later today, tomorrow, later
//! this week, this weekend and next week, each at a set hour of the local
//! day. A choice that has passed, or falls when an earlier one does, is
//! left out.

use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

/// The evening hour of Later today.
const EVENING: i8 = 18;
/// The morning hour of every other choice.
const MORNING: i8 = 8;
/// How long before the evening Later today stops being offered, as Gmail
/// drops it late in the day.
const LATER_TODAY_CUTOFF: i8 = EVENING - 1;

/// One way to snooze: its name, when it ends, and that time as the menu
/// shows it ("Tue, 6:00 PM").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: &'static str,
    pub until: Timestamp,
    pub when: String,
}

/// The choices as of `now`, in the system's time zone.
pub fn choices(now: Timestamp) -> Vec<Choice> {
    choices_in(now, TimeZone::system())
}

fn choices_in(now: Timestamp, tz: TimeZone) -> Vec<Choice> {
    let now = now.to_zoned(tz.clone());
    let today = now.date();
    let in_days = |days: i64| today.checked_add(days.days()).ok();
    let next = |weekday: Weekday| {
        let ahead = weekday.to_monday_zero_offset() - today.weekday().to_monday_zero_offset();
        in_days(i64::from(if ahead <= 0 { ahead + 7 } else { ahead }))
    };
    let mid_week = |date: &Date| {
        matches!(
            date.weekday(),
            Weekday::Tuesday | Weekday::Wednesday | Weekday::Thursday | Weekday::Friday
        )
    };
    let weekday = !matches!(today.weekday(), Weekday::Saturday | Weekday::Sunday);
    let candidates = [
        (
            "Later today",
            (now.hour() < LATER_TODAY_CUTOFF).then_some(today),
            EVENING,
        ),
        ("Tomorrow", in_days(1), MORNING),
        ("Later this week", in_days(2).filter(mid_week), MORNING),
        (
            "This weekend",
            next(Weekday::Saturday).filter(|_| weekday),
            MORNING,
        ),
        ("Next week", next(Weekday::Monday), MORNING),
    ];
    let mut choices: Vec<Choice> = Vec::new();
    for (label, date, hour) in candidates {
        let Some(at) = date.and_then(|date| date.at(hour, 0, 0, 0).to_zoned(tz.clone()).ok())
        else {
            continue;
        };
        let until = at.timestamp();
        if until <= now.timestamp() || choices.iter().any(|choice| choice.until == until) {
            continue;
        }
        choices.push(Choice {
            label,
            until,
            when: at.strftime("%a, %-I:%M %p").to_string(),
        });
    }
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(now: &str) -> Vec<(&'static str, String)> {
        let tz = TimeZone::get("America/Los_Angeles").unwrap();
        let now = jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M", now)
            .unwrap()
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp();
        choices_in(now, tz)
            .into_iter()
            .map(|choice| (choice.label, choice.when))
            .collect()
    }

    #[test]
    fn a_tuesday_morning_offers_what_gmail_does() {
        assert_eq!(
            offered("2026-09-29 10:00"),
            [
                ("Later today", "Tue, 6:00 PM".to_string()),
                ("Tomorrow", "Wed, 8:00 AM".to_string()),
                ("Later this week", "Thu, 8:00 AM".to_string()),
                ("This weekend", "Sat, 8:00 AM".to_string()),
                ("Next week", "Mon, 8:00 AM".to_string()),
            ]
        );
    }

    #[test]
    fn late_in_the_day_later_today_goes() {
        let labels: Vec<_> = offered("2026-09-29 17:30")
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(
            labels,
            ["Tomorrow", "Later this week", "This weekend", "Next week"]
        );
    }

    #[test]
    fn at_the_end_of_the_week_later_this_week_goes_and_friday_s_weekend_is_tomorrow() {
        let thursday: Vec<_> = offered("2026-10-01 09:00")
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(
            thursday,
            ["Later today", "Tomorrow", "This weekend", "Next week"]
        );
        let friday: Vec<_> = offered("2026-10-02 09:00")
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(friday, ["Later today", "Tomorrow", "Next week"]);
    }

    #[test]
    fn on_the_weekend_next_week_is_monday_once() {
        assert_eq!(
            offered("2026-10-03 09:00"),
            [
                ("Later today", "Sat, 6:00 PM".to_string()),
                ("Tomorrow", "Sun, 8:00 AM".to_string()),
                ("Next week", "Mon, 8:00 AM".to_string()),
            ]
        );
        assert_eq!(
            offered("2026-10-04 09:00"),
            [
                ("Later today", "Sun, 6:00 PM".to_string()),
                ("Tomorrow", "Mon, 8:00 AM".to_string()),
                ("Later this week", "Tue, 8:00 AM".to_string()),
            ]
        );
    }

    #[test]
    fn next_week_from_a_monday_is_the_monday_after() {
        let monday = offered("2026-09-28 09:00");
        assert_eq!(
            monday.last(),
            Some(&("Next week", "Mon, 8:00 AM".to_string()))
        );
        assert_eq!(monday[2], ("Later this week", "Wed, 8:00 AM".to_string()));
    }
}

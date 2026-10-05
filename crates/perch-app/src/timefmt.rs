//! Relative timestamps, matching the web UI's wording.

use jiff::{Timestamp, Unit, Zoned};

/// Whole calendar days from `when` to `now` in the local zone.
fn days_between(when: &Zoned, now: &Zoned) -> i64 {
    let a = when.date();
    let b = now.date();
    b.since((Unit::Day, a))
        .map(|s| s.get_days() as i64)
        .unwrap_or(0)
}

/// "42 minutes ago", "yesterday", "3 weeks ago", "Jan 3".
pub fn relative(when: Timestamp, now: Timestamp) -> String {
    let secs = now.as_second() - when.as_second();

    // A clock skew between us and GitHub should not print "in 3 seconds".
    if secs < 45 {
        return "just now".to_string();
    }

    let minutes = secs / 60;
    if minutes < 60 {
        return match minutes {
            0 | 1 => "a minute ago".to_string(),
            m => format!("{m} minutes ago"),
        };
    }

    let hours = minutes / 60;
    if hours < 24 {
        return match hours {
            1 => "an hour ago".to_string(),
            h => format!("{h} hours ago"),
        };
    }

    let tz = jiff::tz::TimeZone::system();
    let when_local = when.to_zoned(tz.clone());
    let now_local = now.to_zoned(tz);
    let days = days_between(&when_local, &now_local);

    match days {
        d if d <= 1 => "yesterday".to_string(),
        2..=6 => format!("{days} days ago"),
        7..=13 => "last week".to_string(),
        14..=29 => format!("{} weeks ago", days / 7),
        _ => {
            let date = when_local.date();
            if date.year() == now_local.date().year() {
                format!("{} {}", month_name(date.month()), date.day())
            } else {
                format!(
                    "{} {}, {}",
                    month_name(date.month()),
                    date.day(),
                    date.year()
                )
            }
        }
    }
}

/// The compact form a drawer row carries at its right edge: "now", "5m",
/// "3h", "1d", "2w", "4mo", "1y". One unit, no "ago", the way Codex's
/// rows read.
pub fn short(when: Timestamp, now: Timestamp) -> String {
    let secs = (now.as_second() - when.as_second()).max(0);
    let minutes = secs / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if minutes < 1 {
        "now".to_string()
    } else if hours < 1 {
        format!("{minutes}m")
    } else if days < 1 {
        format!("{hours}h")
    } else if days < 7 {
        format!("{days}d")
    } else if days < 30 {
        format!("{}w", days / 7)
    } else if days < 365 {
        format!("{}mo", days / 30)
    } else {
        format!("{}y", days / 365)
    }
}

/// When something ahead of `now` happens, as a clock would say it: "3:00
/// PM" later today, "tomorrow 9:00 AM", "Sat 9:58 AM" within the week,
/// "Oct 3" after that.
pub fn ahead(when: Timestamp, now: Timestamp) -> String {
    ahead_in(when, now, jiff::tz::TimeZone::system())
}

fn ahead_in(when: Timestamp, now: Timestamp, tz: jiff::tz::TimeZone) -> String {
    let when = when.to_zoned(tz.clone());
    let now = now.to_zoned(tz);
    let clock = when.strftime("%-I:%M %p").to_string();
    match days_between(&now, &when) {
        d if d <= 0 => clock,
        1 => format!("tomorrow {clock}"),
        2..=6 => format!("{} {clock}", when.strftime("%a")),
        _ => format!("{} {}", month_name(when.month()), when.day()),
    }
}

fn month_name(m: i8) -> &'static str {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    NAMES[((m as usize).clamp(1, 12)) - 1]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn ahead_says_the_time_the_day_or_the_date() {
        let utc = jiff::tz::TimeZone::UTC;
        let now = ts("2026-09-27T17:00:00Z");
        assert_eq!(
            ahead_in(ts("2026-09-27T22:00:00Z"), now, utc.clone()),
            "10:00 PM"
        );
        assert_eq!(
            ahead_in(ts("2026-09-28T09:05:00Z"), now, utc.clone()),
            "tomorrow 9:05 AM"
        );
        assert_eq!(
            ahead_in(ts("2026-10-03T16:58:29Z"), now, utc.clone()),
            "Sat 4:58 PM"
        );
        assert_eq!(ahead_in(ts("2026-10-12T00:00:00Z"), now, utc), "Oct 12");
    }

    #[test]
    fn sub_minute_reads_as_just_now() {
        let now = ts("2026-09-17T12:00:00Z");
        assert_eq!(relative(ts("2026-09-17T11:59:40Z"), now), "just now");
    }

    #[test]
    fn a_clock_running_ahead_does_not_produce_negative_text() {
        let now = ts("2026-09-17T12:00:00Z");
        assert_eq!(relative(ts("2026-09-17T12:00:05Z"), now), "just now");
    }

    #[test]
    fn minutes_and_hours_match_the_web_ui() {
        let now = ts("2026-09-17T12:00:00Z");
        assert_eq!(relative(ts("2026-09-17T11:18:00Z"), now), "42 minutes ago");
        assert_eq!(relative(ts("2026-09-17T11:00:00Z"), now), "an hour ago");
        assert_eq!(relative(ts("2026-09-17T07:00:00Z"), now), "5 hours ago");
    }

    #[test]
    fn singular_minute_reads_naturally() {
        let now = ts("2026-09-17T12:01:00Z");
        assert_eq!(relative(ts("2026-09-17T12:00:00Z"), now), "a minute ago");
    }

    #[test]
    fn weeks_collapse_the_way_github_does() {
        let now = ts("2026-09-17T12:00:00Z");
        assert_eq!(relative(ts("2026-09-09T12:00:00Z"), now), "last week");
        assert_eq!(relative(ts("2026-09-01T12:00:00Z"), now), "2 weeks ago");
        assert_eq!(relative(ts("2026-08-25T12:00:00Z"), now), "3 weeks ago");
    }

    #[test]
    fn beyond_a_month_switches_to_a_date() {
        let now = ts("2026-09-17T12:00:00Z");
        assert_eq!(relative(ts("2026-06-03T12:00:00Z"), now), "Jun 3");
    }

    #[test]
    fn a_previous_year_keeps_the_year() {
        let now = ts("2026-09-17T12:00:00Z");
        assert_eq!(relative(ts("2025-01-03T12:00:00Z"), now), "Jan 3, 2025");
    }

    #[test]
    fn the_short_form_is_one_unit_with_no_ago() {
        let now = ts("2026-09-20T12:00:00Z");
        assert_eq!(short(ts("2026-09-20T11:59:40Z"), now), "now");
        assert_eq!(
            short(ts("2026-09-20T12:00:05Z"), now),
            "now",
            "a clock ahead of ours"
        );
        assert_eq!(short(ts("2026-09-20T11:55:00Z"), now), "5m");
        assert_eq!(short(ts("2026-09-20T09:00:00Z"), now), "3h");
        assert_eq!(short(ts("2026-09-19T11:00:00Z"), now), "1d");
        assert_eq!(short(ts("2026-09-14T12:00:00Z"), now), "6d");
        assert_eq!(short(ts("2026-09-13T12:00:00Z"), now), "1w");
        assert_eq!(short(ts("2026-09-06T12:00:00Z"), now), "2w");
        assert_eq!(short(ts("2026-08-01T12:00:00Z"), now), "1mo");
        assert_eq!(short(ts("2026-01-01T12:00:00Z"), now), "8mo");
        assert_eq!(short(ts("2024-09-01T12:00:00Z"), now), "2y");
    }

    #[test]
    fn month_names_are_in_range() {
        assert_eq!(month_name(1), "Jan");
        assert_eq!(month_name(12), "Dec");
    }
}

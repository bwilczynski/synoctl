//! `--since`/`--until` values: a subset of systemd.time(7), in the local zone.

use jiff::{Span, Timestamp, Zoned, civil};

use crate::error::{Error, Result};

const FORMS: &str =
    "now, today, yesterday, tomorrow, -2h, +30m, \"2h ago\" or YYYY-MM-DD [HH:MM[:SS]]";

/// The instant `input` names, relative to `now` and in `now`'s time zone.
pub fn parse(input: &str, now: &Zoned) -> Result<Timestamp> {
    let invalid = || Error::Other(format!("invalid time {input:?}: use {FORMS}"));
    let s = input.trim();
    let day = |days: i64| {
        now.start_of_day()
            .and_then(|midnight| midnight.checked_add(Span::new().days(days)))
    };
    let zoned = match s {
        "now" => Ok(now.clone()),
        "today" => day(0),
        "yesterday" => day(-1),
        "tomorrow" => day(1),
        _ => {
            if let Some(rest) = s.strip_suffix(" ago") {
                now.checked_sub(span(rest).ok_or_else(invalid)?)
            } else if let Some(rest) = s.strip_prefix('-') {
                now.checked_sub(span(rest).ok_or_else(invalid)?)
            } else if let Some(rest) = s.strip_prefix('+') {
                now.checked_add(span(rest).ok_or_else(invalid)?)
            } else {
                let dt: civil::DateTime = s.parse().map_err(|_| invalid())?;
                dt.to_zoned(now.time_zone().clone())
            }
        }
    };
    zoned.map(|z| z.timestamp()).map_err(|_| invalid())
}

/// `30m`, `1d12h`, `2w`: one or more number-and-unit pairs (s, m, h, d, w).
fn span(s: &str) -> Option<Span> {
    let (mut weeks, mut days, mut hours, mut minutes, mut seconds) = (0i64, 0i64, 0i64, 0i64, 0i64);
    let mut rest = s;
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits == 0 {
            return None;
        }
        let n: i64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let total = match &rest[..unit_len] {
            "w" => &mut weeks,
            "d" => &mut days,
            "h" => &mut hours,
            "m" => &mut minutes,
            "s" => &mut seconds,
            _ => return None,
        };
        *total = total.checked_add(n)?;
        rest = &rest[unit_len..];
    }
    Span::new()
        .try_weeks(weeks)
        .and_then(|s| s.try_days(days))
        .and_then(|s| s.try_hours(hours))
        .and_then(|s| s.try_minutes(minutes))
        .and_then(|s| s.try_seconds(seconds))
        .ok()
}

#[cfg(test)]
mod tests {
    use jiff::{Timestamp, Zoned};

    use super::parse;

    fn at(s: &str) -> Zoned {
        s.parse().unwrap()
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    const NOW: &str = "2026-10-11T12:34:56+02:00[Europe/Warsaw]";

    #[test]
    fn keywords() {
        let now = at(NOW);
        assert_eq!(parse("now", &now).unwrap(), ts("2026-10-11T10:34:56Z"));
        assert_eq!(parse("today", &now).unwrap(), ts("2026-10-10T22:00:00Z"));
        assert_eq!(
            parse("yesterday", &now).unwrap(),
            ts("2026-10-09T22:00:00Z")
        );
        assert_eq!(parse("tomorrow", &now).unwrap(), ts("2026-10-11T22:00:00Z"));
    }

    #[test]
    fn relative() {
        let now = at(NOW);
        assert_eq!(parse("-2h", &now).unwrap(), ts("2026-10-11T08:34:56Z"));
        assert_eq!(parse("2h ago", &now).unwrap(), ts("2026-10-11T08:34:56Z"));
        assert_eq!(parse("+30m", &now).unwrap(), ts("2026-10-11T11:04:56Z"));
        assert_eq!(parse("-90s", &now).unwrap(), ts("2026-10-11T10:33:26Z"));
        assert_eq!(parse("-1d12h", &now).unwrap(), ts("2026-10-09T22:34:56Z"));
        assert_eq!(parse("-1w", &now).unwrap(), ts("2026-10-04T10:34:56Z"));
    }

    #[test]
    fn absolute_in_the_local_zone() {
        let now = at(NOW);
        assert_eq!(
            parse("2026-10-11", &now).unwrap(),
            ts("2026-10-10T22:00:00Z")
        );
        assert_eq!(
            parse("2026-10-11 08:00", &now).unwrap(),
            ts("2026-10-11T06:00:00Z")
        );
        assert_eq!(
            parse("2026-10-11 08:00:05", &now).unwrap(),
            ts("2026-10-11T06:00:05Z")
        );
        assert_eq!(
            parse("2026-10-11T08:00:05", &now).unwrap(),
            ts("2026-10-11T06:00:05Z")
        );
    }

    #[test]
    fn day_arithmetic_follows_dst() {
        // Summer time in Warsaw ended at 03:00 on 2026-10-25.
        let now = at("2026-10-26T12:00:00+01:00[Europe/Warsaw]");
        assert_eq!(parse("-1d", &now).unwrap(), ts("2026-10-25T11:00:00Z"));
        assert_eq!(
            parse("yesterday", &now).unwrap(),
            ts("2026-10-24T22:00:00Z")
        );
    }

    #[test]
    fn rejects_other_forms() {
        let now = at(NOW);
        for s in [
            "",
            "soon",
            "-",
            "-2",
            "-h",
            "2h",
            "-2x",
            "+2h ago",
            "2026-13-01",
            "2026-10-11T08:00:00Z",
        ] {
            let err = parse(s, &now).unwrap_err().to_string();
            assert!(
                err.starts_with(&format!("invalid time {s:?}: use now, today")),
                "{s:?}: {err}"
            );
        }
    }
}

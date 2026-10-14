//! Ported from homelab-cli/internal/output/output.go, by way of unifictl.

use jiff::{Timestamp, tz::TimeZone};

/// Seconds → "1d 0h 0m 0s"; leading zero segments skipped, seconds always shown.
pub fn format_uptime(seconds: i64) -> String {
    let (d, rem) = (seconds / 86400, seconds % 86400);
    let (h, rem) = (rem / 3600, rem % 3600);
    let (m, s) = (rem / 60, rem % 60);
    let mut parts: Vec<String> = Vec::new();
    for (val, unit) in [(d, "d"), (h, "h"), (m, "m"), (s, "s")] {
        if !parts.is_empty() || val > 0 || unit == "s" {
            parts.push(format!("{val}{unit}"));
        }
    }
    parts.join(" ")
}

fn binary_units(n: i64, suffix: &str) -> String {
    const UNIT: i64 = 1024;
    if n < UNIT {
        return format!("{n} B{suffix}");
    }
    let (mut div, mut exp, mut q) = (UNIT, 0usize, n / UNIT);
    while q >= UNIT {
        div *= UNIT;
        exp += 1;
        q /= UNIT;
    }
    const UNITS: [&str; 6] = ["K", "M", "G", "T", "P", "E"];
    format!("{:.1} {}B{suffix}", n as f64 / div as f64, UNITS[exp])
}

pub fn format_bytes(n: i64) -> String {
    binary_units(n, "")
}

pub fn format_bytes_per_sec(n: i64) -> String {
    binary_units(n, "/s")
}

/// "YYYY-MM-DD HH:MM" in `tz` (absolute, so output is stable).
pub fn format_time(t: Timestamp, tz: &TimeZone) -> String {
    t.to_zoned(tz.clone())
        .strftime("%Y-%m-%d %H:%M")
        .to_string()
}

/// "71.5%".
pub fn format_percent(p: f64) -> String {
    format!("{p:.1}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime() {
        assert_eq!(format_uptime(86400), "1d 0h 0m 0s");
        assert_eq!(format_uptime(7200), "2h 0m 0s");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(360000), "4d 4h 0m 0s");
    }

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(11508017246208), "10.5 TB");
        assert_eq!(format_bytes(8230588534784), "7.5 TB");
        assert_eq!(format_bytes(3206674163), "3.0 GB");
        assert_eq!(format_bytes_per_sec(10641), "10.4 KB/s");
        assert_eq!(format_bytes(i64::MAX), "8.0 EB");
    }

    #[test]
    fn time_is_minutes_in_the_given_zone() {
        let t: Timestamp = "2026-04-24T00:30:00Z".parse().unwrap();
        assert_eq!(format_time(t, &TimeZone::UTC), "2026-04-24 00:30");
        let warsaw = TimeZone::get("Europe/Warsaw").unwrap();
        assert_eq!(format_time(t, &warsaw), "2026-04-24 02:30");
    }

    #[test]
    fn percent() {
        assert_eq!(format_percent(71.52), "71.5%");
        assert_eq!(format_percent(0.0), "0.0%");
    }
}

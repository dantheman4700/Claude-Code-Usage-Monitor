//! Just enough calendar arithmetic to find a billing month without a date
//! crate: proleptic-Gregorian conversions (Howard Hinnant's algorithms) and
//! an RFC 3339 formatter.

use std::time::{SystemTime, UNIX_EPOCH};

/// The calendar month containing `now`, on a clock offset from UTC by
/// `day_offset_secs` (Devin's day starts at 08:00 UTC, for instance).
/// Returns the month's first instant and the next month's first instant,
/// both as unix seconds.
pub(super) fn month_bounds(now: SystemTime, day_offset_secs: u64) -> (u64, u64) {
    let now_unix = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let local = now_unix.saturating_sub(day_offset_secs);
    let (year, month, _) = civil_from_days(local / 86_400);
    let start = days_from_civil(year, month, 1) * 86_400 + day_offset_secs;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = days_from_civil(next_year, next_month, 1) * 86_400 + day_offset_secs;
    (start, end)
}

/// "YYYY-MM-DD" for `now` on a clock offset from UTC by `day_offset_secs`.
pub(super) fn date_key(now: SystemTime, day_offset_secs: u64) -> String {
    let unix = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (year, month, day) = civil_from_days(unix.saturating_sub(day_offset_secs) / 86_400);
    format!("{year:04}-{month:02}-{day:02}")
}

pub(super) fn rfc3339(unix: u64) -> String {
    let (year, month, day) = civil_from_days(unix / 86_400);
    let secs = unix % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3_600,
        (secs % 3_600) / 60,
        secs % 60
    )
}

pub(super) fn civil_from_days(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub(super) fn days_from_civil(year: i64, month: u32, day: u32) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn civil_conversions_round_trip() {
        for days in [0, 19_000, 20_692, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(20_692), (2026, 8, 27));
    }

    #[test]
    fn month_bounds_respect_the_day_offset() {
        // 2026-09-01T01:00:00Z is still August on a clock that starts its
        // day at 08:00 UTC.
        let early = UNIX_EPOCH + Duration::from_secs(1_788_220_800 + 3_600); // 2026-09-01T01:00:00Z
        let (start, end) = month_bounds(early, 8 * 3_600);
        assert_eq!(rfc3339(start), "2026-08-01T08:00:00Z");
        assert_eq!(rfc3339(end), "2026-09-01T08:00:00Z");
        let (start, _) = month_bounds(early, 0);
        assert_eq!(rfc3339(start), "2026-09-01T00:00:00Z");
    }
}

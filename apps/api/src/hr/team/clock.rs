//! Local-time windows for "today", "this week" and calendar months, from a fixed UTC offset.

use time::{Date, Month};

pub const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
const UNIX_EPOCH_JULIAN_DAY: i32 = 2_440_588;

/// Days since 1970-01-01 for `YYYY-MM-DD`.
pub fn parse_date(s: &str) -> Option<i64> {
    let mut parts = s.trim().split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let date = Date::from_calendar_date(y.parse().ok()?, Month::try_from(m.parse::<u8>().ok()?).ok()?, d.parse().ok()?).ok()?;
    Some((date.to_julian_day() - UNIX_EPOCH_JULIAN_DAY) as i64)
}

pub fn date_string(day: i64) -> String {
    match Date::from_julian_day(day as i32 + UNIX_EPOCH_JULIAN_DAY) {
        Ok(d) => format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day()),
        Err(_) => "1970-01-01".into(),
    }
}

/// Start of a `YYYY-MM-DD` date at local midnight, as UTC ms.
pub fn date_start_ms(date: &str, tz_min: i64) -> Option<i64> {
    parse_date(date).map(|d| d * DAY_MS - tz_min * 60_000)
}

fn month_parts(month: &str) -> Option<(i32, u8)> {
    let (y, m) = month.split_once('-')?;
    if y.len() != 4 || m.len() != 2 {
        return None;
    }
    let (y, m): (i32, u8) = (y.parse().ok()?, m.parse().ok()?);
    Month::try_from(m).ok().map(|_| (y, m))
}

pub fn valid_month(month: &str) -> bool {
    month_parts(month).is_some()
}

/// `[start, end)` of a `YYYY-MM` month in local time, as UTC ms.
pub fn month_bounds(month: &str, tz_min: i64) -> Option<(i64, i64)> {
    let (y, m) = month_parts(month)?;
    let start = Date::from_calendar_date(y, Month::try_from(m).ok()?, 1).ok()?;
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let next = Date::from_calendar_date(ny, Month::try_from(nm).ok()?, 1).ok()?;
    let to_ms = |d: Date| (d.to_julian_day() - UNIX_EPOCH_JULIAN_DAY) as i64 * DAY_MS - tz_min * 60_000;
    Some((to_ms(start), to_ms(next)))
}

/// `YYYY-MM` of a UTC ms instant in local time.
pub fn month_of(ms: i64, tz_min: i64) -> String {
    let d = date_string((ms + tz_min * 60_000).div_euclid(DAY_MS));
    d[..7].to_string()
}

pub fn previous_month(month: &str) -> String {
    let (y, m) = month_parts(month).unwrap_or((1970, 1));
    if m == 1 { format!("{:04}-12", y - 1) } else { format!("{y:04}-{:02}", m - 1) }
}

#[derive(Debug, Clone)]
pub struct Windows {
    pub day_start: i64,
    pub day_end: i64,
    pub week_start: i64,
    pub week_end: i64,
    pub local_hour: i64,
    /// Monday = 0.
    pub weekday: i64,
    pub date: String,
    pub week_date: String,
    pub month: String,
}

pub fn windows(now: i64, tz_min: i64) -> Windows {
    let local = now + tz_min * 60_000;
    let day = local.div_euclid(DAY_MS);
    let weekday = (day + 3).rem_euclid(7);
    let week_day = day - weekday;
    let to_utc = |d: i64| d * DAY_MS - tz_min * 60_000;
    Windows {
        day_start: to_utc(day),
        day_end: to_utc(day + 1),
        week_start: to_utc(week_day),
        week_end: to_utc(week_day + 7),
        local_hour: local.rem_euclid(DAY_MS) / HOUR_MS,
        weekday,
        date: date_string(day),
        week_date: date_string(week_day),
        month: month_of(now, tz_min),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_windows() {
        assert_eq!(parse_date("1970-01-02"), Some(1));
        assert_eq!(date_string(parse_date("2026-10-14").unwrap()), "2026-10-14");
        assert!(parse_date("2026-13-01").is_none() && parse_date("2026-1-01").is_none());
        // 2026-10-14 is a Wednesday.
        let now = date_start_ms("2026-10-14", 0).unwrap() + 10 * HOUR_MS;
        let w = windows(now, 0);
        assert_eq!((w.weekday, w.local_hour, w.date.as_str(), w.week_date.as_str()), (2, 10, "2026-10-14", "2026-10-12"));
        assert_eq!(w.day_end - w.day_start, DAY_MS);
        let ist = windows(now, 330);
        assert_eq!(ist.local_hour, 15);
        assert_eq!(ist.day_start, date_start_ms("2026-10-14", 330).unwrap());
        let (s, e) = month_bounds("2026-02", 0).unwrap();
        assert_eq!((e - s) / DAY_MS, 28);
        assert_eq!(previous_month("2026-01"), "2025-12");
        assert_eq!(month_of(e, 0), "2026-03");
    }
}

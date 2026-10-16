//! Which time zone a workspace lives in, and how a UTC instant maps to its local day and clock.
//!
//! The zone is the user's own (when one is known), else the workspace's IANA name, else a fixed offset, else `Asia/Kolkata`. An offset of 0
//! counts as unset: it was the column's default, never a choice. IANA rules come from the tz database, so daylight saving is right.

use time::{Date, OffsetDateTime, PrimitiveDateTime, Time};
use time_tz::{OffsetDateTimeExt, PrimitiveDateTimeExt, Tz, timezones};

use crate::hr::team::clock::DAY_MS;

pub const DEFAULT_ZONE: &str = "Asia/Kolkata";
const UNIX_EPOCH_JULIAN_DAY: i32 = 2_440_588;

#[derive(Debug, Clone, Copy)]
pub enum Zone {
    Named(&'static Tz),
    /// A fixed offset from UTC in minutes (no daylight saving).
    Fixed(i64),
}

/// A UTC instant seen on a local wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    /// Days since 1970-01-01 of the local date.
    pub day: i64,
    pub minute_of_day: i64,
}

fn named(name: &str) -> Option<&'static Tz> {
    let name = name.trim();
    (!name.is_empty() && name.len() <= 64).then(|| timezones::get_by_name(name)).flatten()
}

/// Is this a zone name the tz database knows?
pub fn is_known(name: &str) -> bool {
    named(name).is_some()
}

/// The zone to count time in. `user` is the person's own IANA name (an invalid one is ignored), `workspace` the workspace's, and
/// `offset_minutes` the older fixed offset, where 0 means "never set".
pub fn resolve(user: Option<&str>, workspace: Option<&str>, offset_minutes: i64) -> Zone {
    if let Some(tz) = user.and_then(named).or_else(|| workspace.and_then(named)) {
        return Zone::Named(tz);
    }
    if offset_minutes != 0 && (-840..=840).contains(&offset_minutes) {
        return Zone::Fixed(offset_minutes);
    }
    Zone::Named(named(DEFAULT_ZONE).expect("the tz database has Asia/Kolkata"))
}

fn utc(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

impl Zone {
    /// Minutes east of UTC at `at_ms`.
    pub fn offset_minutes(&self, at_ms: i64) -> i64 {
        match self {
            Zone::Fixed(m) => *m,
            Zone::Named(tz) => i64::from(utc(at_ms).to_timezone(*tz).offset().whole_seconds()) / 60,
        }
    }

    pub fn local(&self, at_ms: i64) -> Local {
        let shifted = at_ms + self.offset_minutes(at_ms) * 60_000;
        Local { day: shifted.div_euclid(DAY_MS), minute_of_day: shifted.rem_euclid(DAY_MS) / 60_000 }
    }

    /// The UTC instant of a local day (days since 1970-01-01) and minute of the day. A time that does not exist (the clock jumped
    /// forward) is taken a little later; one that happens twice is taken the first time.
    pub fn to_utc(&self, day: i64, minute_of_day: i64) -> i64 {
        let naive = day * DAY_MS + minute_of_day * 60_000;
        match self {
            Zone::Fixed(m) => naive - m * 60_000,
            Zone::Named(tz) => {
                let total = minute_of_day.clamp(0, 24 * 60 - 1);
                let date = i32::try_from(day).ok().and_then(|d| Date::from_julian_day(d + UNIX_EPOCH_JULIAN_DAY).ok());
                let clock = Time::from_hms((total / 60) as u8, (total % 60) as u8, 0).ok();
                let (Some(date), Some(clock)) = (date, clock) else { return naive };
                let primitive = PrimitiveDateTime::new(date, clock);
                let at = primitive.assume_timezone(*tz);
                let found = at.take_first().or_else(|| primitive.saturating_add(time::Duration::hours(1)).assume_timezone(*tz).take_first());
                match found {
                    Some(dt) => (dt.unix_timestamp() * 1000) + (minute_of_day - total) * 60_000,
                    None => naive,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset(z: Zone) -> i64 {
        z.offset_minutes(1_791_900_000_000)
    }

    #[test]
    fn zero_offset_counts_as_unset_and_falls_back_to_kolkata() {
        assert_eq!(offset(resolve(None, None, 0)), 330, "never set: Kolkata");
        assert_eq!(offset(resolve(None, None, -300)), -300, "a real fixed offset is kept");
        assert_eq!(offset(resolve(None, Some("not/a-zone"), 0)), 330, "an invalid name is ignored");
        assert_eq!(offset(resolve(Some("America/New_York"), Some("Asia/Kolkata"), 330)), -240, "the user's own zone wins, daylight saving included in October");
        assert_eq!(offset(resolve(Some("bogus"), Some("Europe/London"), 0)), 60);
    }

    #[test]
    fn local_time_and_back_agree_across_daylight_saving() {
        let ny = resolve(Some("America/New_York"), None, 0);
        // 2026-03-08 07:30 UTC is 03:30 in New York (the clock jumped at 02:00).
        let at = 1_772_955_000_000;
        let local = ny.local(at);
        assert_eq!(local.minute_of_day, 3 * 60 + 30);
        assert_eq!(ny.to_utc(local.day, local.minute_of_day), at);
        let kolkata = resolve(None, None, 0);
        let there = kolkata.local(at);
        assert_eq!(kolkata.to_utc(there.day, there.minute_of_day), at);
    }
}

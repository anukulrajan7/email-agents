//! Working time in a workspace's own zone. Storage stays UTC; this module only answers "when is 4 working hours from now" and
//! writes a date the way a person says it. Zone resolution and the working-minutes maths live here; the human time words for the
//! assistant ("yesterday at 3:30 PM") come in a later slice.

pub mod zone;

pub use zone::{DEFAULT_ZONE, Local, Zone, resolve};

/// Days of the week are numbered Monday = 0.
pub const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// When a workspace works: whole hours of the day, and which days.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkWeek {
    pub start_hour: i64,
    pub end_hour: i64,
    /// Monday = 0 ... Sunday = 6.
    pub days: [bool; 7],
}

impl WorkWeek {
    /// Nine to seven, Monday to Saturday (most Indian staffing offices work Saturdays).
    pub fn standard() -> WorkWeek {
        WorkWeek { start_hour: 9, end_hour: 19, days: [true, true, true, true, true, true, false] }
    }

    pub fn from_days(start_hour: i64, end_hour: i64, days: &[i64]) -> WorkWeek {
        let mut set = [false; 7];
        for d in days.iter().filter(|d| (0..7).contains(*d)) {
            set[*d as usize] = true;
        }
        // A week with no working day would never end a countdown.
        if !set.iter().any(|d| *d) {
            return WorkWeek { start_hour, end_hour, ..WorkWeek::standard() };
        }
        WorkWeek { start_hour, end_hour, days: set }
    }

    /// Working minutes in one working day.
    pub fn day_minutes(&self) -> i64 {
        (self.end_hour - self.start_hour) * 60
    }

    fn is_valid(&self) -> bool {
        (0..24).contains(&self.start_hour) && (1..=24).contains(&self.end_hour) && self.start_hour < self.end_hour && self.days.iter().any(|d| *d)
    }
}

/// The instant `minutes` working minutes after `from` (UTC ms): nights and non-working days do not count. The same instant when
/// `minutes` is 0 and `from` is inside working time. An invalid week (no hours, no days) falls back to the standard one.
pub fn add_working_minutes(zone: &Zone, week: &WorkWeek, from_ms: i64, minutes: i64) -> i64 {
    let standard = WorkWeek::standard();
    let week = if week.is_valid() { week } else { &standard };
    let mut left = minutes.max(0);
    let here = zone.local(from_ms);
    let (mut day, mut minute) = (here.day, here.minute_of_day);
    for _ in 0..800 {
        if week.days[weekday(day) as usize] {
            let from_minute = minute.max(week.start_hour * 60);
            let room = (week.end_hour * 60 - from_minute).max(0);
            if room > 0 && left <= room {
                return zone.to_utc(day, from_minute + left);
            }
            left -= room;
        }
        day += 1;
        minute = 0;
    }
    zone.to_utc(day, 0)
}

/// Monday = 0 for a day counted from 1970-01-01 (a Thursday).
pub fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

/// "Friday 17 Oct": the date a person is told, in the zone's own calendar.
pub fn day_label(zone: &Zone, at_ms: i64) -> String {
    let local = zone.local(at_ms);
    let iso = crate::hr::team::clock::date_string(local.day);
    let month: usize = iso[5..7].parse().unwrap_or(1);
    let date: u32 = iso[8..10].parse().unwrap_or(1);
    format!("{} {date} {}", WEEKDAYS[weekday(local.day) as usize], MONTHS[month.clamp(1, 12) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hr::team::clock::{DAY_MS, parse_date};

    const MIN: i64 = 60_000;

    fn kolkata() -> Zone {
        resolve(None, Some("Asia/Kolkata"), 0)
    }

    /// UTC ms of a local Kolkata date and time.
    fn ist(date: &str, hour: i64, minute: i64) -> i64 {
        parse_date(date).unwrap() * DAY_MS + (hour * 60 + minute) * MIN - 330 * MIN
    }

    #[test]
    fn working_minutes_skip_sunday_and_nights() {
        let (zone, week) = (kolkata(), WorkWeek::standard());
        // Saturday 17 Oct 2026 at 17:00: two hours left that day, Sunday does not count, Monday 9:00 starts the rest.
        let sat = ist("2026-10-17", 17, 0);
        assert_eq!(add_working_minutes(&zone, &week, sat, 240), ist("2026-10-19", 11, 0));
        // After hours, the clock starts at the next morning.
        assert_eq!(add_working_minutes(&zone, &week, ist("2026-10-14", 22, 0), 240), ist("2026-10-15", 13, 0));
        assert_eq!(add_working_minutes(&zone, &week, ist("2026-10-14", 6, 0), 60), ist("2026-10-14", 10, 0), "before opening too");
        assert_eq!(add_working_minutes(&zone, &week, ist("2026-10-14", 10, 0), 0), ist("2026-10-14", 10, 0));
    }

    #[test]
    fn a_working_day_is_the_hours_between_start_and_end() {
        let (zone, week) = (kolkata(), WorkWeek::standard());
        assert_eq!(week.day_minutes(), 600);
        assert_eq!(add_working_minutes(&zone, &week, ist("2026-10-14", 9, 0), 600), ist("2026-10-14", 19, 0));
        assert_eq!(add_working_minutes(&zone, &week, ist("2026-10-14", 15, 0), 600), ist("2026-10-15", 15, 0));
        assert_eq!(add_working_minutes(&zone, &WorkWeek::from_days(9, 19, &[]), ist("2026-10-18", 10, 0), 60), ist("2026-10-19", 10, 0), "no working day at all falls back to the standard week");
    }

    #[test]
    fn a_date_is_written_the_way_a_person_says_it() {
        assert_eq!(day_label(&kolkata(), ist("2026-10-16", 12, 0)), "Friday 16 Oct");
        assert_eq!(day_label(&kolkata(), ist("2026-10-17", 23, 59)), "Saturday 17 Oct", "late evening is still that day in Kolkata");
    }
}

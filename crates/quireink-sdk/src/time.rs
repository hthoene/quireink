//! The time of day: time zones with their summer time rules, and the local
//! time shown e.g. in a status line.
//!
//! Shells report the wall clock and, where the system has one, its time
//! zone (see [`SystemStatus`](crate::SystemStatus)). The setting
//! `core.time-zone` uses the system's zone ("automatic", the default) or
//! one of [`TIME_ZONES`]; `core.time-zone-rule` takes any POSIX `TZ` rule
//! (e.g. `CET-1CEST,M3.5.0,M10.5.0/3`) for zones not in the list. Rules are
//! a few bytes each, so no time zone database is needed.

use alloc::format;
use alloc::string::String;

use crate::builtin::settings;
use crate::env::Env;
use quireink_platform::Instant;

/// The automatic time zone: the system's, or UTC without one.
pub const AUTO: &str = "core.tz.auto";

/// Time zones to choose from: an id (its translation is the name shown)
/// and its POSIX `TZ` rule.
pub const TIME_ZONES: &[(&str, &str)] = &[
    ("core.tz.utc", "UTC0"),
    ("core.tz.honolulu", "HST10"),
    ("core.tz.anchorage", "AKST9AKDT,M3.2.0,M11.1.0"),
    ("core.tz.los-angeles", "PST8PDT,M3.2.0,M11.1.0"),
    ("core.tz.phoenix", "MST7"),
    ("core.tz.denver", "MST7MDT,M3.2.0,M11.1.0"),
    ("core.tz.chicago", "CST6CDT,M3.2.0,M11.1.0"),
    ("core.tz.new-york", "EST5EDT,M3.2.0,M11.1.0"),
    ("core.tz.sao-paulo", "<-03>3"),
    ("core.tz.london", "GMT0BST,M3.5.0/1,M10.5.0"),
    ("core.tz.lisbon", "WET0WEST,M3.5.0/1,M10.5.0"),
    ("core.tz.berlin", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("core.tz.helsinki", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("core.tz.istanbul", "<+03>-3"),
    ("core.tz.moscow", "MSK-3"),
    ("core.tz.dubai", "<+04>-4"),
    ("core.tz.india", "IST-5:30"),
    ("core.tz.bangkok", "<+07>-7"),
    ("core.tz.china", "CST-8"),
    ("core.tz.tokyo", "JST-9"),
    ("core.tz.sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("core.tz.auckland", "NZST-12NZDT,M9.5.0,M4.1.0/3"),
];

/// A date and time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 0 (Sunday) to 6 (Saturday).
    pub weekday: u8,
}

impl LocalTime {
    /// The local time `seconds` after 1970-01-01 00:00 local time.
    pub fn from_seconds(seconds: i64) -> Self {
        let days = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        LocalTime {
            year,
            month,
            day,
            hour: (rest / 3600) as u8,
            minute: (rest / 60 % 60) as u8,
            second: (rest % 60) as u8,
            weekday: (days + 4).rem_euclid(7) as u8,
        }
    }
}

/// A POSIX time zone rule: the standard offset and, optionally, summer time
/// with its start and end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeZone {
    /// Seconds east of UTC.
    standard: i32,
    summer: Option<Summer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summer {
    offset: i32,
    start: Rule,
    end: Rule,
}

/// `Mm.w.d/time`: weekday `d` (0 = Sunday) of week `w` (5 = the last) of
/// month `m`, at `time` seconds local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rule {
    month: u8,
    week: u8,
    weekday: u8,
    time: i32,
}

impl TimeZone {
    /// Parses a POSIX `TZ` rule such as `CET-1CEST,M3.5.0,M10.5.0/3`.
    /// Only `Mm.w.d` dates are supported, as used by every common zone.
    pub fn parse(rule: &str) -> Option<TimeZone> {
        let mut p = Parser(rule.trim().as_bytes());
        p.name()?;
        let standard = -p.offset()?;
        if p.done() {
            return Some(TimeZone {
                standard,
                summer: None,
            });
        }
        p.name()?;
        let offset = if p.peek() == Some(b',') {
            standard + 3600
        } else {
            -p.offset()?
        };
        p.expect(b',')?;
        let start = p.rule()?;
        p.expect(b',')?;
        let end = p.rule()?;
        p.done().then_some(TimeZone {
            standard,
            summer: Some(Summer { offset, start, end }),
        })
    }

    /// The offset from UTC, in seconds, at `unix` seconds since 1970.
    pub fn offset_at(&self, unix: i64) -> i32 {
        let Some(summer) = self.summer else {
            return self.standard;
        };
        let year = LocalTime::from_seconds(unix + i64::from(self.standard)).year;
        // Start is given in standard time, end in summer time.
        let start = summer.start.local_seconds(year) - i64::from(self.standard);
        let end = summer.end.local_seconds(year) - i64::from(summer.offset);
        let in_summer = if start < end {
            start <= unix && unix < end
        } else {
            // Southern hemisphere: summer spans the new year.
            !(end <= unix && unix < start)
        };
        if in_summer {
            summer.offset
        } else {
            self.standard
        }
    }
}

impl Rule {
    /// When the rule applies in `year`, in seconds since 1970 local time.
    fn local_seconds(&self, year: i32) -> i64 {
        let first = days_from_civil(year, self.month, 1);
        let first_weekday = (first + 4).rem_euclid(7) as u8;
        let mut day = 1 + (self.weekday + 7 - first_weekday) % 7 + (self.week - 1) * 7;
        while day > days_in_month(year, self.month) {
            day -= 7;
        }
        (first + i64::from(day) - 1) * 86_400 + i64::from(self.time)
    }
}

struct Parser<'a>(&'a [u8]);

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    fn done(&self) -> bool {
        self.0.is_empty()
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.0 = &self.0[1..])
    }

    /// A zone name: letters, or anything in angle brackets.
    fn name(&mut self) -> Option<()> {
        if self.peek()? == b'<' {
            let end = self.0.iter().position(|&b| b == b'>')?;
            self.0 = &self.0[end + 1..];
            return Some(());
        }
        let len = self
            .0
            .iter()
            .take_while(|b| b.is_ascii_alphabetic())
            .count();
        (len >= 3).then(|| self.0 = &self.0[len..])
    }

    fn number(&mut self) -> Option<i32> {
        let len = self.0.iter().take_while(|b| b.is_ascii_digit()).count();
        if len == 0 || len > 3 {
            return None;
        }
        let value = self.0[..len]
            .iter()
            .fold(0, |n, &b| n * 10 + i32::from(b - b'0'));
        self.0 = &self.0[len..];
        Some(value)
    }

    /// `[+-]hh[:mm[:ss]]` in seconds.
    fn offset(&mut self) -> Option<i32> {
        let sign = match self.peek()? {
            b'-' => {
                self.0 = &self.0[1..];
                -1
            }
            b'+' => {
                self.0 = &self.0[1..];
                1
            }
            _ => 1,
        };
        let mut seconds = self.number()? * 3600;
        for unit in [60, 1] {
            if self.peek() != Some(b':') {
                break;
            }
            self.0 = &self.0[1..];
            seconds += self.number()? * unit;
        }
        Some(sign * seconds)
    }

    /// `Mm.w.d[/time]`.
    fn rule(&mut self) -> Option<Rule> {
        self.expect(b'M')?;
        let month = self.number()?;
        self.expect(b'.')?;
        let week = self.number()?;
        self.expect(b'.')?;
        let weekday = self.number()?;
        let time = if self.peek() == Some(b'/') {
            self.0 = &self.0[1..];
            self.offset()?
        } else {
            7200
        };
        let valid = (1..=12).contains(&month) && (1..=5).contains(&week) && weekday <= 6;
        valid.then_some(Rule {
            month: month as u8,
            week: week as u8,
            weekday: weekday as u8,
            time,
        })
    }
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a date (proleptic Gregorian calendar).
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (year, month, day)
}

/// The time zone chosen in the settings; `None` for the system's.
fn chosen_zone(env: &Env<'_>) -> Option<TimeZone> {
    let rule = env.settings.text(settings::TIME_ZONE_RULE.as_str()).trim();
    if !rule.is_empty() {
        return TimeZone::parse(rule);
    }
    let zone = env.settings.text(settings::TIME_ZONE.as_str());
    TIME_ZONES
        .iter()
        .find(|(id, _)| *id == zone)
        .and_then(|(_, rule)| TimeZone::parse(rule))
}

/// Local milliseconds since 1970 at `now`, if the device knows the time.
fn local_millis(env: &Env<'_>, now: Instant) -> Option<i64> {
    let clock = env.status.clock?;
    let utc = clock.unix_millis_at(now);
    let offset = match chosen_zone(env) {
        Some(zone) => i64::from(zone.offset_at(utc.div_euclid(1000))) * 1000,
        None => i64::from(clock.utc_offset_minutes.unwrap_or(0)) * 60_000,
    };
    Some(utc + offset)
}

/// Seconds since 1970 (UTC) now, if the device knows the time.
pub fn unix_seconds(env: &Env<'_>) -> Option<i64> {
    let clock = env.status.clock?;
    Some(clock.unix_millis_at(env.now).div_euclid(1000))
}

/// `unix` seconds since 1970 (UTC) as local seconds since 1970, in the
/// chosen time zone (or the system's); see [`LocalTime::from_seconds`].
pub fn local_seconds(env: &Env<'_>, unix: i64) -> i64 {
    let offset = match chosen_zone(env) {
        Some(zone) => i64::from(zone.offset_at(unix)),
        None => env
            .status
            .clock
            .and_then(|c| c.utc_offset_minutes)
            .map_or(0, |m| i64::from(m) * 60),
    };
    unix + offset
}

/// The local date and time now, if the device knows the time.
pub fn local_time(env: &Env<'_>) -> Option<LocalTime> {
    local_millis(env, env.now).map(|ms| LocalTime::from_seconds(ms.div_euclid(1000)))
}

/// When the minute changes next, e.g. to update a clock.
pub fn next_minute(env: &Env<'_>) -> Option<Instant> {
    let ms = local_millis(env, env.now)?;
    Some(env.now.add_millis((60_000 - ms.rem_euclid(60_000)) as u64))
}

/// The time as the user wants to see it: `14:05` or `2:05 PM`
/// (setting `core.clock-format`).
pub fn clock_text(env: &Env<'_>) -> Option<String> {
    let t = local_time(env)?;
    if env.settings.text(settings::CLOCK_FORMAT.as_str()) == CLOCK_12H {
        let hour = match t.hour % 12 {
            0 => 12,
            h => h,
        };
        let key = if t.hour < 12 {
            "core.clock.am"
        } else {
            "core.clock.pm"
        };
        let time = format!("{hour}:{:02}", t.minute);
        Some(
            env.i18n
                .format(key, &[("time", quireink_i18n::Arg::Str(&time))]),
        )
    } else {
        Some(format!("{:02}:{:02}", t.hour, t.minute))
    }
}

/// The values of `core.clock-format`.
pub const CLOCK_24H: &str = "core.clock.24h";
pub const CLOCK_12H: &str = "core.clock.12h";

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(year: i32, month: u8, day: u8, hour: i64) -> i64 {
        days_from_civil(year, month, day) * 86_400 + hour * 3600
    }

    #[test]
    fn dates_round_trip() {
        for days in [-1, 0, 59, 365, 11_016, 20_000, 60_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        let t = LocalTime::from_seconds(utc(2026, 10, 14, 13) + 5 * 60);
        assert_eq!(
            (t.year, t.month, t.day, t.hour, t.minute),
            (2026, 10, 14, 13, 5)
        );
        assert_eq!(t.weekday, 3);
    }

    #[test]
    fn summer_time_in_berlin() {
        let berlin = TimeZone::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        // 2026: summer time from 29 March 01:00 UTC to 25 October 01:00 UTC.
        assert_eq!(berlin.offset_at(utc(2026, 1, 10, 12)), 3600);
        assert_eq!(berlin.offset_at(utc(2026, 3, 29, 0)), 3600);
        assert_eq!(berlin.offset_at(utc(2026, 3, 29, 1)), 7200);
        assert_eq!(berlin.offset_at(utc(2026, 10, 25, 0)), 7200);
        assert_eq!(berlin.offset_at(utc(2026, 10, 25, 1)), 3600);
    }

    #[test]
    fn southern_and_fixed_zones() {
        let sydney = TimeZone::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(sydney.offset_at(utc(2026, 1, 10, 0)), 11 * 3600);
        assert_eq!(sydney.offset_at(utc(2026, 7, 10, 0)), 10 * 3600);
        let india = TimeZone::parse("IST-5:30").unwrap();
        assert_eq!(india.offset_at(0), 5 * 3600 + 1800);
        let sao_paulo = TimeZone::parse("<-03>3").unwrap();
        assert_eq!(sao_paulo.offset_at(0), -3 * 3600);
        assert!(TimeZone::parse("nonsense").is_none());
        for (id, rule) in TIME_ZONES {
            assert!(TimeZone::parse(rule).is_some(), "{id}");
        }
    }
}

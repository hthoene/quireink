//! Durations, dates and days as the views show them.

use alloc::format;
use alloc::string::{String, ToString};

use quireink_sdk::time::{self, LocalTime};
use quireink_sdk::{Arg, Env};

const DAY: i64 = 86_400;

/// The local day (days since 1970) of a moment in Unix seconds.
pub(crate) fn day_of(env: &Env<'_>, unix: u32) -> i32 {
    time::local_seconds(env, i64::from(unix)).div_euclid(DAY) as i32
}

/// Today, if the device knows the time.
pub(crate) fn today(env: &Env<'_>) -> Option<i32> {
    let now = time::unix_seconds(env)?;
    Some(time::local_seconds(env, now).div_euclid(DAY) as i32)
}

/// The date of a local day.
pub(crate) fn date_of(day: i32) -> LocalTime {
    LocalTime::from_seconds(i64::from(day) * DAY)
}

/// The Unix second around which local day `day` starts (exact up to the
/// time zone's offset, which is at most a day).
pub(crate) fn unix_near(day: i32) -> i64 {
    i64::from(day) * DAY
}

/// `1 h 05 min`, `42 min` or `< 1 min`.
pub(crate) fn duration(env: &Env<'_>, seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes == 0 {
        if seconds == 0 {
            return env.i18n.format("stats.minutes", &[("m", Arg::Int(0))]);
        }
        return env.i18n.tr("stats.under-a-minute").to_string();
    }
    if minutes < 60 {
        return env
            .i18n
            .format("stats.minutes", &[("m", Arg::Int(minutes as i64))]);
    }
    env.i18n.format(
        "stats.hours",
        &[
            ("h", Arg::Int((minutes / 60) as i64)),
            ("m", Arg::Str(&format!("{:02}", minutes % 60))),
        ],
    )
}

/// `14.10.2026` or `10/14/2026`, as the language writes dates.
pub(crate) fn date(env: &Env<'_>, day: i32) -> String {
    let d = date_of(day);
    env.i18n.format(
        "stats.date",
        &[
            ("day", Arg::Int(i64::from(d.day))),
            ("month", Arg::Int(i64::from(d.month))),
            ("year", Arg::Int(i64::from(d.year))),
        ],
    )
}

/// `Dienstag, 14. Oktober`.
pub(crate) fn long_date(env: &Env<'_>, day: i32) -> String {
    let d = date_of(day);
    env.i18n.format(
        "stats.long-date",
        &[
            ("weekday", Arg::Str(weekday(env, d.weekday))),
            ("day", Arg::Int(i64::from(d.day))),
            ("month", Arg::Str(month(env, d.month))),
        ],
    )
}

/// The name of a weekday, 0 being Sunday.
pub(crate) fn weekday<'a>(env: &Env<'a>, weekday: u8) -> &'a str {
    const KEYS: [&str; 7] = [
        "stats.weekday.0",
        "stats.weekday.1",
        "stats.weekday.2",
        "stats.weekday.3",
        "stats.weekday.4",
        "stats.weekday.5",
        "stats.weekday.6",
    ];
    env.i18n.tr(KEYS[usize::from(weekday % 7)])
}

/// The short name of a weekday (two letters), 0 being Sunday.
pub(crate) fn weekday_short<'a>(env: &Env<'a>, weekday: u8) -> &'a str {
    const KEYS: [&str; 7] = [
        "stats.weekday-short.0",
        "stats.weekday-short.1",
        "stats.weekday-short.2",
        "stats.weekday-short.3",
        "stats.weekday-short.4",
        "stats.weekday-short.5",
        "stats.weekday-short.6",
    ];
    env.i18n.tr(KEYS[usize::from(weekday % 7)])
}

/// The name of a month, 1 being January.
pub(crate) fn month<'a>(env: &Env<'a>, month: u8) -> &'a str {
    const KEYS: [&str; 12] = [
        "stats.month.1",
        "stats.month.2",
        "stats.month.3",
        "stats.month.4",
        "stats.month.5",
        "stats.month.6",
        "stats.month.7",
        "stats.month.8",
        "stats.month.9",
        "stats.month.10",
        "stats.month.11",
        "stats.month.12",
    ];
    env.i18n.tr(KEYS[usize::from(month.clamp(1, 12) - 1)])
}

/// The first day of the week: 1 for Monday (most of Europe), 0 for
/// Sunday, as the language has it.
pub(crate) fn week_start(env: &Env<'_>) -> u8 {
    if env.i18n.tr("stats.week-start") == "0" {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_have_dates() {
        // 2025-10-14 was a Tuesday.
        let day = 1_760_400_000 / 86_400;
        let d = date_of(day);
        assert_eq!((d.year, d.month, d.day, d.weekday), (2025, 10, 14, 2));
        assert_eq!(unix_near(day), i64::from(day) * 86_400);
    }
}

//! The battery log: a line for each reading, as CSV, readable by people
//! and spreadsheets alike. A time awake starts with a `start` line (at
//! start-up or waking), has a `sample` line each time the battery is
//! measured (about once a minute) and ends with an `end` line (going to
//! sleep). Besides the battery, each line has what the device did since
//! QuireInk started ([`Usage`]) and the pages turned, so the battery's use
//! can be put down to what used it.
//!
//! What each plugin took goes into [`PLUGINS`], every ten minutes awake
//! and at sleep: the time its code ran and its screens were shown, the
//! refreshes then, and the storage it read and wrote, each since start-up.
//!
//! Each log is kept to [`MAX_BYTES`]: then it becomes its `.old` (replacing
//! the one before) and a new one starts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::usage::{PluginUsage, Usage};

/// The log, in the plugin's folder.
pub(crate) const FILE: &str = "log.csv";
/// The log before.
pub(crate) const OLD: &str = "log.old.csv";
/// What each plugin took.
pub(crate) const PLUGINS: &str = "plugins.csv";
/// Its first line.
pub(crate) const PLUGINS_HEADER: &str =
    "ms,plugin,busy_ms,calls,shown_ms,fast,partial,full,kilopixels,read_kb,written_kb\n";
/// How often what each plugin took is noted, awake.
pub(crate) const PLUGINS_EVERY_MS: u64 = 10 * 60 * 1000;
/// Size at which a new log starts.
pub(crate) const MAX_BYTES: u64 = 128 * 1024;
/// The first line of a log.
pub(crate) const HEADER: &str =
    "kind,ms,millivolts,percent,charging,fast,partial,full,kilopixels,inputs,panel_ms,pages,unix\n";
/// Times awake kept for the view.
const KEPT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Start,
    Sample,
    End,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Start => "start",
            Kind::Sample => "sample",
            Kind::End => "end",
        }
    }
}

/// What the device did since start-up, of its [`Usage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Counts {
    pub refreshes: [u32; 3],
    pub pixels: u64,
    pub inputs: u32,
    pub panel_ms: u64,
}

impl Counts {
    pub fn of(usage: &Usage) -> Self {
        Self {
            refreshes: usage.refreshes,
            pixels: usage.pixels,
            inputs: usage.inputs,
            panel_ms: usage.panel_ms,
        }
    }

    pub fn total_refreshes(&self) -> u32 {
        self.refreshes
            .iter()
            .fold(0u32, |a, &b| a.saturating_add(b))
    }
}

/// A reading: the battery and what was done, at `ms` since start-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Reading {
    pub ms: u64,
    pub millivolts: Option<u16>,
    pub percent: Option<u8>,
    pub charging: bool,
    pub usage: Counts,
    pub pages: u32,
    /// The time of day, as Unix seconds, where the device knows it.
    pub unix: Option<i64>,
}

/// The line of a reading.
pub(crate) fn line(kind: Kind, r: &Reading) -> String {
    let opt = |v: Option<String>| v.unwrap_or_default();
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
        kind.name(),
        r.ms,
        opt(r.millivolts.map(|v| format!("{v}"))),
        opt(r.percent.map(|v| format!("{v}"))),
        u8::from(r.charging),
        r.usage.refreshes[0],
        r.usage.refreshes[1],
        r.usage.refreshes[2],
        r.usage.pixels / 1000,
        r.usage.inputs,
        r.usage.panel_ms,
        r.pages,
        opt(r.unix.map(|v| format!("{v}"))),
    )
}

/// A line read back; `None` for the header and anything else.
pub(crate) fn parse(line: &str) -> Option<(Kind, Reading)> {
    let mut fields = line.trim_end().split(',');
    let kind = match fields.next()? {
        "start" => Kind::Start,
        "sample" => Kind::Sample,
        "end" => Kind::End,
        _ => return None,
    };
    let mut next = || fields.next();
    let ms = next()?.parse().ok()?;
    let millivolts = next()?.parse().ok();
    let percent = next()?.parse().ok();
    let charging = next()? == "1";
    let mut usage = Counts::default();
    for refreshes in usage.refreshes.iter_mut() {
        *refreshes = next()?.parse().ok()?;
    }
    usage.pixels = next()?.parse::<u64>().ok()? * 1000;
    usage.inputs = next()?.parse().ok()?;
    usage.panel_ms = next()?.parse().ok()?;
    let pages = next()?.parse().ok()?;
    let unix = next().and_then(|v| v.parse().ok());
    Some((
        kind,
        Reading {
            ms,
            millivolts,
            percent,
            charging,
            usage,
            pages,
            unix,
        },
    ))
}

/// The lines of what each plugin took by `ms`, for those that took
/// anything.
pub(crate) fn plugin_lines(ms: u64, plugins: &[PluginUsage]) -> String {
    let mut text = String::new();
    for p in plugins {
        let busy = p.busy_us / 1000;
        if p.calls == 0 && p.shown_ms == 0 && p.read == 0 && p.written == 0 {
            continue;
        }
        text.push_str(&format!(
            "{ms},{},{busy},{},{},{},{},{},{},{},{}\n",
            p.plugin,
            p.calls,
            p.shown_ms,
            p.refreshes[0],
            p.refreshes[1],
            p.refreshes[2],
            p.pixels / 1000,
            p.read / 1024,
            p.written / 1024,
        ));
    }
    text
}

/// A time awake: its first and last readings of the battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Session {
    pub first: Reading,
    pub last: Reading,
    /// It charged at some time.
    pub charged: bool,
}

impl Session {
    fn new(r: Reading) -> Self {
        Self {
            first: r,
            last: r,
            charged: r.charging,
        }
    }

    /// Takes a reading; the first with the battery's charge starts it.
    fn add(&mut self, r: Reading) {
        if self.first.percent.is_none() {
            self.first = r;
        }
        if r.percent.is_some() || self.last.percent.is_none() {
            self.last = r;
        }
        self.charged |= r.charging;
    }

    pub fn millis(&self) -> u64 {
        self.last.ms.saturating_sub(self.first.ms)
    }

    /// Percent used (negative: it gained).
    pub fn used(&self) -> Option<i32> {
        Some(i32::from(self.first.percent?) - i32::from(self.last.percent?))
    }

    pub fn pages(&self) -> u32 {
        self.last.pages.saturating_sub(self.first.pages)
    }

    pub fn refreshes(&self) -> u32 {
        self.last
            .usage
            .total_refreshes()
            .saturating_sub(self.first.usage.total_refreshes())
    }

    /// On battery all along, and long enough to say something.
    pub fn counts(&self) -> bool {
        !self.charged && self.millis() >= MIN_MILLIS && self.used().is_some_and(|u| u >= 0)
    }
}

/// Times awake shorter than this say nothing about the battery.
const MIN_MILLIS: u64 = 5 * 60 * 1000;

/// Times awake read from the log, line by line.
#[derive(Debug, Default)]
pub(crate) struct Sessions {
    /// Ended ones, the latest last.
    pub ended: Vec<Session>,
    /// The one going on.
    pub current: Option<Session>,
}

impl Sessions {
    pub fn feed(&mut self, kind: Kind, r: Reading) {
        match kind {
            Kind::Start => {
                self.close();
                self.current = Some(Session::new(r));
            }
            Kind::Sample | Kind::End => {
                match &mut self.current {
                    // A reading in another start-up (the counters began
                    // anew without a start line): a new time awake.
                    Some(s) if r.ms < s.last.ms => {
                        self.close();
                        self.current = Some(Session::new(r));
                    }
                    Some(s) => s.add(r),
                    None => self.current = Some(Session::new(r)),
                }
                if kind == Kind::End {
                    self.close();
                }
            }
        }
    }

    fn close(&mut self) {
        if let Some(session) = self.current.take() {
            if self.ended.len() == KEPT {
                self.ended.remove(0);
            }
            self.ended.push(session);
        }
    }

    /// Reads `text`, a log's lines.
    pub fn read_text(&mut self, text: &str) {
        for line in text.lines() {
            if let Some((kind, reading)) = parse(line) {
                self.feed(kind, reading);
            }
        }
    }

    /// Reads the old log, then the log, from `dir`, a piece at a time.
    pub fn read(storage: &dyn Storage, dir: &str) -> Self {
        let mut sessions = Self::default();
        for name in [OLD, FILE] {
            let path = format!("{dir}/{name}");
            let mut at = 0u64;
            let mut buf = [0u8; 2048];
            let mut rest: Vec<u8> = Vec::new();
            while let Ok(n) = storage.read_at(&path, at, &mut buf) {
                if n == 0 {
                    break;
                }
                at += n as u64;
                rest.extend_from_slice(&buf[..n]);
                // Whole lines; the rest waits for its end.
                if let Some(end) = rest.iter().rposition(|&b| b == b'\n') {
                    if let Ok(text) = core::str::from_utf8(&rest[..=end]) {
                        sessions.read_text(text);
                    }
                    rest.drain(..=end);
                }
            }
            if let Ok(text) = core::str::from_utf8(&rest) {
                sessions.read_text(text);
            }
        }
        sessions
    }

    /// The times awake that count (see [`Session::counts`]), the current
    /// one too.
    pub fn counting(&self) -> impl Iterator<Item = &Session> {
        self.ended
            .iter()
            .chain(self.current.as_ref())
            .filter(|s| s.counts())
    }

    /// Use on battery, all times awake together.
    pub fn totals(&self) -> Totals {
        let mut totals = Totals::default();
        for s in self.counting() {
            totals.millis += s.millis();
            totals.used += s.used().unwrap_or(0) as u32;
            totals.pages += s.pages();
        }
        totals
    }
}

/// Use on battery over many times awake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Totals {
    pub millis: u64,
    /// Percent used.
    pub used: u32,
    pub pages: u32,
}

impl Totals {
    /// Percent used an hour awake, once there is enough to say it.
    pub fn per_hour(&self) -> Option<f32> {
        (self.used >= 2 && self.millis >= 30 * 60 * 1000)
            .then(|| self.used as f32 * 3_600_000.0 / self.millis as f32)
    }

    /// Percent used for 100 pages, once enough were turned.
    pub fn per_100_pages(&self) -> Option<f32> {
        (self.used >= 2 && self.pages >= 50).then(|| self.used as f32 * 100.0 / self.pages as f32)
    }

    /// Hours awake left with `percent`, at the pace so far.
    pub fn hours_left(&self, percent: u8) -> Option<f32> {
        self.per_hour()
            .filter(|rate| *rate > 0.0)
            .map(|rate| f32::from(percent) / rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(ms: u64, percent: u8, pages: u32) -> Reading {
        Reading {
            ms,
            millivolts: Some(3700 + u16::from(percent) * 4),
            percent: Some(percent),
            pages,
            ..Reading::default()
        }
    }

    #[test]
    fn lines_read_back() {
        let mut r = reading(61_000, 87, 12);
        r.usage.refreshes = [3, 10, 1];
        r.usage.pixels = 4_800_000;
        r.usage.inputs = 30;
        r.usage.panel_ms = 9000;
        r.unix = Some(1_791_967_260);
        let text = line(Kind::Sample, &r);
        assert_eq!(
            text,
            "sample,61000,4048,87,0,3,10,1,4800,30,9000,12,1791967260\n"
        );
        assert_eq!(parse(&text), Some((Kind::Sample, r)));
        // Unknown parts stay empty.
        let r = Reading {
            ms: 5,
            ..Reading::default()
        };
        assert_eq!(line(Kind::Start, &r), "start,5,,,0,0,0,0,0,0,0,0,\n");
        assert_eq!(parse(&line(Kind::Start, &r)), Some((Kind::Start, r)));
        assert_eq!(parse(HEADER), None);
    }

    #[test]
    fn plugin_lines_for_those_that_took_anything() {
        let plugins = [
            PluginUsage {
                plugin: quireink_sdk::Id::new_static("reader"),
                busy_us: 2_500_000,
                calls: 40,
                shown_ms: 600_000,
                refreshes: [5, 30, 2],
                pixels: 12_000_000,
                read: 300 * 1024,
                written: 2048,
            },
            PluginUsage {
                plugin: quireink_sdk::Id::new_static("idle"),
                ..PluginUsage::default()
            },
        ];
        assert_eq!(
            plugin_lines(61_000, &plugins),
            "61000,reader,2500,40,600000,5,30,2,12000,300,2\n"
        );
    }

    #[test]
    fn times_awake_and_their_use() {
        let minute = 60_000;
        let mut log = String::from(HEADER);
        // An hour on battery: 90 % to 85 %, 100 pages.
        log.push_str(&line(Kind::Start, &Reading::default()));
        for m in 0..=60 {
            let percent = 90 - (m / 12) as u8;
            log.push_str(&line(
                Kind::Sample,
                &reading(m * minute, percent, m as u32 * 5 / 3),
            ));
        }
        log.push_str(&line(Kind::End, &reading(60 * minute, 85, 100)));
        // Charging: not counted.
        log.push_str(&line(Kind::Start, &reading(0, 85, 0)));
        let mut charging = reading(30 * minute, 95, 0);
        charging.charging = true;
        log.push_str(&line(Kind::Sample, &charging));
        log.push_str(&line(Kind::End, &charging));
        // Woken without a start line, the counters anew: another time.
        log.push_str(&line(Kind::Sample, &reading(10 * minute, 80, 0)));
        log.push_str(&line(Kind::Sample, &reading(2 * minute, 80, 0)));
        let mut sessions = Sessions::default();
        sessions.read_text(&log);
        assert_eq!(sessions.ended.len(), 3);
        let first = sessions.ended[0];
        assert_eq!(first.millis(), 60 * minute);
        assert_eq!(first.used(), Some(5));
        assert_eq!(first.pages(), 100);
        assert!(first.counts());
        assert!(!sessions.ended[1].counts());
        assert!(sessions.current.is_some());
        let totals = sessions.totals();
        assert_eq!(totals.used, 5);
        assert_eq!(totals.per_hour(), Some(5.0));
        assert_eq!(totals.per_100_pages(), Some(5.0));
        assert_eq!(totals.hours_left(80), Some(16.0));
    }
}

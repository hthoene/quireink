//! Rules: when something happens, do something, if a condition holds.
//! Kept in the plugin's folder, one rule a line:
//! `trigger<TAB>effect<TAB>target<TAB>on|off<TAB>condition` (no condition:
//! always).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::{Env, Id, Storage};

use crate::PLUGIN;

/// Days of the week as bits, Sunday the lowest (as `LocalTime::weekday`).
pub(crate) const EVERY_DAY: u8 = 0b111_1111;
/// Monday to Friday.
pub(crate) const WEEKDAYS: u8 = 0b011_1110;
/// Saturday and Sunday.
pub(crate) const WEEKENDS: u8 = 0b100_0001;

/// When a rule applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// QuireInk has started.
    Start,
    /// At this minute of the day, on the days set ([`EVERY_DAY`] …).
    Time {
        minute: u16,
        days: u8,
    },
    /// Every so many minutes, from the start.
    Every(u16),
    /// A book was opened, closed, read to its last page.
    BookOpened,
    BookClosed,
    BookFinished,
    /// The device woke up, or goes to sleep.
    Resume,
    Sleep,
    /// The battery's charge fell below this percentage.
    BatteryBelow(u8),
    /// Charging began, or ended.
    Charging,
    Unplugged,
    /// A switch (an on/off setting) was turned on, or off.
    SwitchOn(Id),
    SwitchOff(Id),
}

/// What a rule does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Runs an action, e.g. `opds.sync`.
    Action(Id),
    /// Turns a switch (an on/off setting) on or off.
    Switch(Id, bool),
    /// Sets a number (a setting with a range), e.g. the light to 20.
    Number(Id, i64),
    /// Shows a message.
    Message(String),
}

/// When a rule may apply at all, besides its trigger.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum Condition {
    #[default]
    Always,
    /// Between two minutes of the day (over midnight when `from` is
    /// later than `to`).
    Between {
        from: u16,
        to: u16,
    },
    /// While a book is open, or while none is.
    Reading,
    NotReading,
    /// While the device runs on its battery, or while it charges.
    OnBattery,
    Charging,
    /// While a switch is on, or off.
    SwitchOn(Id),
    SwitchOff(Id),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub trigger: Trigger,
    pub effect: Effect,
    /// Rules can be paused without being removed.
    pub on: bool,
    pub condition: Condition,
}

impl Condition {
    /// As written in the file: `between 22:00 06:00`, `reading`,
    /// `not-reading`, `battery`, `charging`, `switch-on <id>`,
    /// `switch-off <id>`; empty for always.
    fn text(&self) -> String {
        match self {
            Condition::Always => String::new(),
            Condition::Between { from, to } => format!("between {} {}", clock(*from), clock(*to)),
            Condition::Reading => String::from("reading"),
            Condition::NotReading => String::from("not-reading"),
            Condition::OnBattery => String::from("battery"),
            Condition::Charging => String::from("charging"),
            Condition::SwitchOn(id) => format!("switch-on {id}"),
            Condition::SwitchOff(id) => format!("switch-off {id}"),
        }
    }

    fn parse(text: &str) -> Option<Condition> {
        let text = text.trim();
        let (word, rest) = text.split_once(' ').unwrap_or((text, ""));
        let rest = rest.trim();
        Some(match word {
            "" | "always" => Condition::Always,
            "between" => {
                let (from, to) = rest.split_once(' ')?;
                Condition::Between {
                    from: parse_time(from)?,
                    to: parse_time(to)?,
                }
            }
            "reading" => Condition::Reading,
            "not-reading" => Condition::NotReading,
            "battery" => Condition::OnBattery,
            "charging" => Condition::Charging,
            "switch-on" => Condition::SwitchOn(Id::parse(rest).ok()?),
            "switch-off" => Condition::SwitchOff(Id::parse(rest).ok()?),
            _ => return None,
        })
    }
}

/// Whether `minute` of the day lies between `from` and `to` (`to` not
/// included; over midnight when `from` is later).
pub(crate) fn between(minute: u16, from: u16, to: u16) -> bool {
    if from <= to {
        (from..to).contains(&minute)
    } else {
        minute >= from || minute < to
    }
}

impl Trigger {
    /// As written in the file: `start`, `time 22:00`, `time 07:00 62`,
    /// `every 30`, `book`, `book-closed`, `book-finished`, `resume`,
    /// `sleep`, `battery-below 20`, `charging`, `unplugged`,
    /// `switch-on <id>`, `switch-off <id>`.
    fn text(&self) -> String {
        match self {
            Trigger::Start => String::from("start"),
            Trigger::Time { minute, days } if *days == EVERY_DAY => {
                format!("time {}", clock(*minute))
            }
            Trigger::Time { minute, days } => format!("time {} {days}", clock(*minute)),
            Trigger::Every(minutes) => format!("every {minutes}"),
            Trigger::BookOpened => String::from("book"),
            Trigger::BookClosed => String::from("book-closed"),
            Trigger::BookFinished => String::from("book-finished"),
            Trigger::Resume => String::from("resume"),
            Trigger::Sleep => String::from("sleep"),
            Trigger::BatteryBelow(percent) => format!("battery-below {percent}"),
            Trigger::Charging => String::from("charging"),
            Trigger::Unplugged => String::from("unplugged"),
            Trigger::SwitchOn(id) => format!("switch-on {id}"),
            Trigger::SwitchOff(id) => format!("switch-off {id}"),
        }
    }

    fn parse(text: &str) -> Option<Trigger> {
        let text = text.trim();
        let (word, rest) = text.split_once(' ').unwrap_or((text, ""));
        let rest = rest.trim();
        Some(match word {
            "start" => Trigger::Start,
            "book" => Trigger::BookOpened,
            "book-closed" => Trigger::BookClosed,
            "book-finished" => Trigger::BookFinished,
            "resume" => Trigger::Resume,
            "sleep" => Trigger::Sleep,
            "charging" => Trigger::Charging,
            "unplugged" => Trigger::Unplugged,
            "time" => {
                let (time, days) = rest.split_once(' ').unwrap_or((rest, ""));
                let days = match days.trim() {
                    "" => EVERY_DAY,
                    days => days.parse::<u8>().ok().filter(|d| d & EVERY_DAY != 0)? & EVERY_DAY,
                };
                Trigger::Time {
                    minute: parse_time(time)?,
                    days,
                }
            }
            "every" => Trigger::Every(rest.parse::<u16>().ok().filter(|&m| m > 0)?),
            "battery-below" => Trigger::BatteryBelow(
                rest.parse::<u8>()
                    .ok()
                    .filter(|&p| (1..=100).contains(&p))?,
            ),
            "switch-on" => Trigger::SwitchOn(Id::parse(rest).ok()?),
            "switch-off" => Trigger::SwitchOff(Id::parse(rest).ok()?),
            _ => return None,
        })
    }
}

/// `22:00` for minute 1320 of the day.
pub(crate) fn clock(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// The minute of the day for `22:00` (also `22.00`, `7:5`, `22`).
pub(crate) fn parse_time(text: &str) -> Option<u16> {
    let text = text.trim();
    let (hours, minutes) = text.split_once([':', '.', 'h']).unwrap_or((text, "0"));
    let hours: u16 = hours.trim().parse().ok()?;
    let minutes: u16 = minutes.trim().parse().ok()?;
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

fn path() -> String {
    format!("{}/rules.txt", Env::plugin_dir(PLUGIN.as_str()))
}

pub(crate) fn load(storage: &dyn Storage) -> Vec<Rule> {
    let data = quireink_sdk::read_file(storage, &path(), 64 * 1024).unwrap_or_default();
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let trigger = Trigger::parse(fields.next()?)?;
            let kind = fields.next()?;
            let target = fields.next()?;
            let id = || Id::parse(target.trim()).ok();
            let effect = match kind {
                "action" => Effect::Action(id()?),
                "switch-on" => Effect::Switch(id()?, true),
                "switch-off" => Effect::Switch(id()?, false),
                "set" => {
                    let (id, value) = target.split_once('=')?;
                    Effect::Number(Id::parse(id.trim()).ok()?, value.trim().parse().ok()?)
                }
                "message" => Effect::Message(target.to_string()),
                _ => return None,
            };
            let on = fields.next().is_none_or(|f| f.trim() != "off");
            let condition = Condition::parse(fields.next().unwrap_or_default())?;
            Some(Rule {
                trigger,
                effect,
                on,
                condition,
            })
        })
        .collect()
}

pub(crate) fn save(storage: &dyn Storage, rules: &[Rule]) {
    let mut text = String::new();
    for rule in rules {
        let (kind, target) = match &rule.effect {
            Effect::Action(id) => ("action", id.to_string()),
            Effect::Switch(id, true) => ("switch-on", id.to_string()),
            Effect::Switch(id, false) => ("switch-off", id.to_string()),
            Effect::Number(id, value) => ("set", format!("{id}={value}")),
            // One line, no fields within it.
            Effect::Message(text) => ("message", text.replace(['\t', '\n', '\r'], " ")),
        };
        let mut line = format!(
            "{}\t{kind}\t{target}\t{}",
            rule.trigger.text(),
            if rule.on { "on" } else { "off" }
        );
        if rule.condition != Condition::Always {
            line.push('\t');
            line.push_str(&rule.condition.text());
        }
        text.push_str(&line);
        text.push('\n');
    }
    let _ = storage.write(&path(), text.as_bytes());
}

/// Minutes from `now` (minute of the day) until `at`: the next time the
/// clock shows `at`, today or tomorrow; never 0.
pub(crate) fn minutes_until(now: u16, at: u16) -> u16 {
    let ahead = (at + 24 * 60 - now) % (24 * 60);
    if ahead == 0 { 24 * 60 } else { ahead }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn rules_are_kept() {
        let storage = MemoryStorage::new();
        assert!(load(&storage).is_empty());
        let night = Id::new_static("core.night-mode");
        let rules: Vec<Rule> = [
            Trigger::Time {
                minute: 22 * 60,
                days: EVERY_DAY,
            },
            Trigger::Time {
                minute: 7 * 60,
                days: WEEKDAYS,
            },
            Trigger::Start,
            Trigger::Every(30),
            Trigger::BookOpened,
            Trigger::BookClosed,
            Trigger::BookFinished,
            Trigger::Resume,
            Trigger::Sleep,
            Trigger::BatteryBelow(20),
            Trigger::Charging,
            Trigger::Unplugged,
            Trigger::SwitchOn(night.clone()),
            Trigger::SwitchOff(night.clone()),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, trigger)| Rule {
            trigger,
            effect: match i % 4 {
                0 => Effect::Switch(night.clone(), true),
                1 => Effect::Action(Id::new_static("opds.sync")),
                2 => Effect::Number(Id::new_static("core.frontlight"), -3),
                _ => Effect::Message(String::from("Time to sleep")),
            },
            on: i % 3 != 0,
            condition: match i % 7 {
                0 => Condition::Always,
                1 => Condition::Between {
                    from: 22 * 60,
                    to: 6 * 60,
                },
                2 => Condition::Reading,
                3 => Condition::NotReading,
                4 => Condition::OnBattery,
                5 => Condition::Charging,
                _ => Condition::SwitchOff(night.clone()),
            },
        })
        .collect();
        save(&storage, &rules);
        assert_eq!(load(&storage), rules);
        // Written before days existed: every day.
        assert_eq!(
            Trigger::parse("time 22:00"),
            Some(Trigger::Time {
                minute: 1320,
                days: EVERY_DAY
            })
        );
        assert_eq!(Trigger::parse("battery-below 0"), None);
        assert_eq!(Trigger::parse("every 0"), None);
        assert_eq!(Trigger::parse("time 22:00 0"), None);
        // Written before conditions and messages: always; tabs in a
        // message do not split the line.
        save(
            &storage,
            &[Rule {
                trigger: Trigger::Start,
                effect: Effect::Message(String::from("a\tb")),
                on: true,
                condition: Condition::SwitchOn(night.clone()),
            }],
        );
        assert_eq!(
            load(&storage)[0].effect,
            Effect::Message(String::from("a b"))
        );
        assert_eq!(Condition::parse(""), Some(Condition::Always));
        assert_eq!(Condition::parse("between 22:00"), None);
    }

    #[test]
    fn times_between() {
        assert!(between(23 * 60, 22 * 60, 6 * 60));
        assert!(between(60, 22 * 60, 6 * 60));
        assert!(!between(12 * 60, 22 * 60, 6 * 60));
        assert!(between(9 * 60, 8 * 60, 17 * 60));
        assert!(!between(17 * 60, 8 * 60, 17 * 60));
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("22:00"), Some(1320));
        assert_eq!(parse_time(" 7.05 "), Some(425));
        assert_eq!(parse_time("7"), Some(420));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("abc"), None);
        assert_eq!(clock(425), "07:05");
        assert_eq!(minutes_until(1300, 1320), 20);
        assert_eq!(minutes_until(1330, 1320), 24 * 60 - 10);
        assert_eq!(minutes_until(1320, 1320), 24 * 60, "just done: tomorrow");
    }
}

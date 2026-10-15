//! Rules: when something happens, do something. Kept in the plugin's
//! folder, one rule a line: `trigger<TAB>effect<TAB>target<TAB>on|off`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::{Env, Id, Storage};

use crate::PLUGIN;

/// When a rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// QuireInk has started.
    Start,
    /// Every day at this minute of the day.
    Time(u16),
    /// A book was opened.
    BookOpened,
    /// The device woke up.
    Resume,
}

/// What a rule does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Runs an action, e.g. `opds.sync`.
    Action(Id),
    /// Turns a switch (an on/off setting) on or off.
    Switch(Id, bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub trigger: Trigger,
    pub effect: Effect,
    /// Rules can be paused without being removed.
    pub on: bool,
}

impl Trigger {
    /// As written in the file: `start`, `time 22:00`, `book`, `resume`.
    fn text(self) -> String {
        match self {
            Trigger::Start => String::from("start"),
            Trigger::Time(minute) => format!("time {}", clock(minute)),
            Trigger::BookOpened => String::from("book"),
            Trigger::Resume => String::from("resume"),
        }
    }

    fn parse(text: &str) -> Option<Trigger> {
        match text.trim() {
            "start" => Some(Trigger::Start),
            "book" => Some(Trigger::BookOpened),
            "resume" => Some(Trigger::Resume),
            other => parse_time(other.strip_prefix("time")?).map(Trigger::Time),
        }
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
            let target = Id::parse(fields.next()?.trim()).ok()?;
            let effect = match kind {
                "action" => Effect::Action(target),
                "switch-on" => Effect::Switch(target, true),
                "switch-off" => Effect::Switch(target, false),
                _ => return None,
            };
            let on = fields.next().is_none_or(|f| f.trim() != "off");
            Some(Rule {
                trigger,
                effect,
                on,
            })
        })
        .collect()
}

pub(crate) fn save(storage: &dyn Storage, rules: &[Rule]) {
    let mut text = String::new();
    for rule in rules {
        let (kind, target) = match &rule.effect {
            Effect::Action(id) => ("action", id),
            Effect::Switch(id, true) => ("switch-on", id),
            Effect::Switch(id, false) => ("switch-off", id),
        };
        text.push_str(&format!(
            "{}\t{kind}\t{target}\t{}\n",
            rule.trigger.text(),
            if rule.on { "on" } else { "off" }
        ));
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
        let rules = [
            Rule {
                trigger: Trigger::Time(22 * 60),
                effect: Effect::Switch(Id::new_static("core.night-mode"), true),
                on: true,
            },
            Rule {
                trigger: Trigger::Start,
                effect: Effect::Action(Id::new_static("opds.sync")),
                on: false,
            },
        ];
        save(&storage, &rules);
        assert_eq!(load(&storage), rules);
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

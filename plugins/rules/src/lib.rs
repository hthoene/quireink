//! Rules: when something happens, QuireInk does something. For example
//! "every day at 22:00: night mode on", "on weekdays at 7:00: sync the
//! catalogs", "when the battery falls below 20 %: the light off".
//!
//! - When: QuireInk starts; at a time of day (every day, on weekdays, at
//!   weekends or on one day); every so many minutes; a book is opened,
//!   closed or read to its end; the device wakes up or goes to sleep; the
//!   battery falls below a percentage; charging begins or ends; a switch
//!   (any on/off setting) is turned on or off.
//! - Then: run any action (from any plugin), or turn any switch on or
//!   off.
//!
//! The rules are listed under Settings › Customise › Rules: tapping one
//! pauses it, holding it removes it, *Add rule* asks when and what. Times
//! come through the kernel's timer for plugins (`Command::Schedule`), so
//! they apply whatever screen is open.

#![no_std]

extern crate alloc;

mod rule;
mod screens;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::builtin::{events, menus, params};
use quireink_sdk::ui::choice::Choice;
use quireink_sdk::{
    ActionSpec, Arg, Command, Env, Id, Manifest, MenuEntry, Params, Plugin, RegisterError,
    Registrar, SDK_VERSION, ScreenSpec, SettingValue, Version, time,
};

use crate::rule::{EVERY_DAY, Effect, Rule, Trigger, WEEKDAYS, WEEKENDS};

pub const PLUGIN: Id = Id::new_static("rules");
/// Opens the list of rules.
pub const OPEN: Id = Id::new_static("rules.open");
const LIST_SCREEN: Id = Id::new_static("rules.list");
const PICK_SCREEN: Id = Id::new_static("rules.pick");
const CHOICE: Id = Id::new_static("rules.choice");
/// The timer of the rules with a time of day; parameter [`MINUTE`].
const TIME: Id = Id::new_static("rules.time");
const MINUTE: &str = "minute";
/// The timer of the rules that repeat: each minute while there are some.
const TICK: Id = Id::new_static("rules.tick");

/// The names of the days, Sunday first.
pub(crate) const DAY_NAMES: [&str; 7] = [
    "rules.day.0",
    "rules.day.1",
    "rules.day.2",
    "rules.day.3",
    "rules.day.4",
    "rules.day.5",
    "rules.day.6",
];

/// Runs the rules whose trigger `applies`.
fn run(env: &Env<'_>, out: &mut Vec<Command>, applies: impl Fn(&Trigger) -> bool) {
    for rule in rule::load(env.storage) {
        if !rule.on || !applies(&rule.trigger) {
            continue;
        }
        match rule.effect {
            Effect::Action(action) => out.push(Command::RunAction(action)),
            Effect::Switch(id, on) => {
                if env.settings.get(id.as_str()) != Some(&SettingValue::Bool(on)) {
                    out.push(Command::SetSetting(id, SettingValue::Bool(on)));
                }
            }
        }
    }
}

/// Sets the timers: to the next time of day a rule waits for, and to the
/// next minute while rules repeat.
fn schedule(env: &Env<'_>, out: &mut Vec<Command>) {
    let Some(now) = time::local_time(env) else {
        return;
    };
    let rules = rule::load(env.storage);
    let minute_now = u16::from(now.hour) * 60 + u16::from(now.minute);
    // The start of a minute so many minutes ahead.
    let at_minute = |ahead: u16| {
        env.now
            .add_millis(u64::from(ahead) * 60_000 - u64::from(now.second) * 1000)
    };
    let next = rules
        .iter()
        .filter(|r| r.on)
        .filter_map(|r| match r.trigger {
            Trigger::Time { minute, .. } => Some((rule::minutes_until(minute_now, minute), minute)),
            _ => None,
        })
        .min();
    if let Some((ahead, at)) = next {
        out.push(Command::Schedule(
            TIME,
            Params::new().with(MINUTE, at.to_string()),
            at_minute(ahead),
        ));
    }
    if rules
        .iter()
        .any(|r| r.on && matches!(r.trigger, Trigger::Every(_)))
    {
        out.push(Command::Schedule(TICK, Params::new(), at_minute(1)));
    }
}

/// The days of a time of day, in words: "every day", "Monday to Friday".
fn days_text(env: &Env<'_>, days: u8) -> String {
    let key = match days {
        EVERY_DAY => "rules.days.every",
        WEEKDAYS => "rules.days.weekdays",
        WEEKENDS => "rules.days.weekends",
        _ => {
            let names: Vec<&str> = (0..7)
                .filter(|d| days & (1 << d) != 0)
                .map(|d| env.i18n.tr(DAY_NAMES[d]))
                .collect();
            return names.join(", ");
        }
    };
    env.i18n.tr(key).to_string()
}

/// The name of a setting.
fn setting_title<'e>(env: &Env<'e>, id: &'e Id) -> &'e str {
    let title = env
        .settings
        .specs()
        .find(|s| s.id == *id)
        .map_or(id.as_str(), |s| s.title.as_ref());
    env.i18n.tr(title)
}

/// When a rule applies, in words.
fn trigger_text(env: &Env<'_>, trigger: &Trigger) -> String {
    let tr = |key: &str| env.i18n.tr(key).to_string();
    match trigger {
        Trigger::Start => tr("rules.when.start"),
        Trigger::Time { minute, days } => {
            let time = rule::clock(*minute);
            let days = days_text(env, *days);
            env.i18n.format(
                "rules.when.time",
                &[("time", Arg::Str(&time)), ("days", Arg::Str(&days))],
            )
        }
        Trigger::Every(minutes) => env.i18n.plural(
            "rules.when.every",
            i64::from(*minutes),
            &[("count", Arg::Int(i64::from(*minutes)))],
        ),
        Trigger::BookOpened => tr("rules.when.book"),
        Trigger::BookClosed => tr("rules.when.book-closed"),
        Trigger::BookFinished => tr("rules.when.book-finished"),
        Trigger::Resume => tr("rules.when.resume"),
        Trigger::Sleep => tr("rules.when.sleep"),
        Trigger::BatteryBelow(percent) => env.i18n.format(
            "rules.when.battery",
            &[("percent", Arg::Int(i64::from(*percent)))],
        ),
        Trigger::Charging => tr("rules.when.charging"),
        Trigger::Unplugged => tr("rules.when.unplugged"),
        Trigger::SwitchOn(id) => env.i18n.format(
            "rules.when.switch-on",
            &[("setting", Arg::Str(setting_title(env, id)))],
        ),
        Trigger::SwitchOff(id) => env.i18n.format(
            "rules.when.switch-off",
            &[("setting", Arg::Str(setting_title(env, id)))],
        ),
    }
}

/// What a rule does, in words.
fn effect_text(env: &Env<'_>, effect: &Effect) -> String {
    match effect {
        Effect::Action(id) => {
            let title = env
                .actions
                .get(id.as_str())
                .map_or(id.as_str(), |a| a.title.as_ref());
            env.i18n.tr(title).to_string()
        }
        Effect::Switch(id, on) => {
            let state = env.i18n.tr(if *on { "rules.on" } else { "rules.off" });
            env.i18n.format(
                "rules.do.switch",
                &[
                    ("setting", Arg::Str(setting_title(env, id))),
                    ("state", Arg::Str(state)),
                ],
            )
        }
    }
}

/// The rules plugin.
#[derive(Debug, Default)]
pub struct Rules;

impl Plugin for Rules {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Rules"),
            description: Cow::Borrowed(
                "When something happens, does something: e.g. night mode at 22:00.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: LIST_SCREEN,
            title: Cow::Borrowed("rules.title"),
            factory: Box::new(|env, _| Box::new(screens::RulesScreen::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: PICK_SCREEN,
            title: Cow::Borrowed("rules.title"),
            factory: Box::new(|env, params| Box::new(screens::PickScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("rules.title"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("rules.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(LIST_SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::SETTINGS,
            item: OPEN,
            order: 230,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("clock"))?;
        r.subscribe(
            events::STARTED,
            Box::new(|env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                run(env, out, |t| *t == Trigger::Start);
                schedule(env, out);
            }),
        )?;
        for (event, trigger) in [
            (events::BOOK_OPENED, Trigger::BookOpened),
            (events::BOOK_CLOSED, Trigger::BookClosed),
            (events::BOOK_FINISHED, Trigger::BookFinished),
            (events::SUSPEND, Trigger::Sleep),
        ] {
            r.subscribe(
                event,
                Box::new(move |env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                    run(env, out, |t| *t == trigger);
                }),
            )?;
        }
        r.subscribe(
            events::RESUME,
            Box::new(|env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                run(env, out, |t| *t == Trigger::Resume);
                // The clock may have moved on while asleep.
                schedule(env, out);
            }),
        )?;
        r.subscribe(
            TIME,
            Box::new(|env: &Env<'_>, params: &Params, out: &mut Vec<Command>| {
                let today = time::local_time(env).map_or(0, |t| t.weekday);
                if let Some(at) = params.get(MINUTE).and_then(|m| m.parse::<u16>().ok()) {
                    run(env, out, |t| {
                        matches!(t, Trigger::Time { minute, days }
                            if *minute == at && days & (1 << today) != 0)
                    });
                }
                schedule(env, out);
            }),
        )?;
        // Minutes since the start, for the rules that repeat.
        let minutes = Rc::new(Cell::new(0u32));
        r.subscribe(
            TICK,
            Box::new(move |env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                let now = minutes.get() + 1;
                minutes.set(now);
                run(
                    env,
                    out,
                    |t| matches!(t, Trigger::Every(every) if now % u32::from(*every) == 0),
                );
                schedule(env, out);
            }),
        )?;
        // The battery as it was last: a charge falling below a mark, and
        // charging beginning or ending, compare with it.
        let battery: Rc<Cell<Option<(u8, bool)>>> = Rc::new(Cell::new(None));
        r.subscribe(
            events::STATUS_CHANGED,
            Box::new(move |env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                let Some(now) = env.status.battery.map(|b| (b.percent, b.charging)) else {
                    return;
                };
                let Some((was, was_charging)) = battery.replace(Some(now)) else {
                    return;
                };
                run(env, out, |t| match t {
                    Trigger::BatteryBelow(mark) => was >= *mark && now.0 < *mark,
                    Trigger::Charging => !was_charging && now.1,
                    Trigger::Unplugged => was_charging && !now.1,
                    _ => false,
                });
            }),
        )?;
        r.subscribe(
            events::SETTING_CHANGED,
            Box::new(|env: &Env<'_>, params: &Params, out: &mut Vec<Command>| {
                let Some(id) = params.get(params::ID) else {
                    return;
                };
                let Some(SettingValue::Bool(on)) = env.settings.get(id) else {
                    return;
                };
                let on = *on;
                run(env, out, |t| match t {
                    Trigger::SwitchOn(s) => on && s.as_str() == id,
                    Trigger::SwitchOff(s) => !on && s.as_str() == id,
                    _ => false,
                });
            }),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Adds `rule` and sets the timers again.
fn add(env: &Env<'_>, rule: Rule, out: &mut Vec<Command>) {
    let mut rules = rule::load(env.storage);
    rules.push(rule);
    rule::save(env.storage, &rules);
    schedule(env, out);
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

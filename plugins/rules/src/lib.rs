//! Rules: when something happens, QuireInk does something. For example
//! "every day at 22:00: night mode on", "when QuireInk starts: sync the
//! catalogs", "when the device wakes up: run an action".
//!
//! - When: QuireInk starts, every day at a time, a book is opened, the
//!   device wakes up.
//! - Then: run any action (from any plugin), or turn any switch (an on/off
//!   setting) on or off.
//!
//! The rules are listed under Settings › Customise › Rules: tapping one
//! pauses it, holding it removes it, *Add rule* asks when and what. Times
//! of day come through the kernel's timer for plugins
//! (`Command::Schedule`), so they apply whatever screen is open.

#![no_std]

extern crate alloc;

mod rule;
mod screens;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{events, menus};
use quireink_sdk::ui::choice::Choice;
use quireink_sdk::{
    ActionSpec, Arg, Command, Env, Id, Manifest, MenuEntry, Params, Plugin, RegisterError,
    Registrar, SDK_VERSION, ScreenSpec, SettingValue, Version, time,
};

use crate::rule::{Effect, Rule, Trigger};

pub const PLUGIN: Id = Id::new_static("rules");
/// Opens the list of rules.
pub const OPEN: Id = Id::new_static("rules.open");
const LIST_SCREEN: Id = Id::new_static("rules.list");
const PICK_SCREEN: Id = Id::new_static("rules.pick");
const CHOICE: Id = Id::new_static("rules.choice");
/// The timer of the rules with a time of day; parameter [`MINUTE`].
const TIME: Id = Id::new_static("rules.time");
const MINUTE: &str = "minute";

/// Runs the rules for `trigger`.
fn run(env: &Env<'_>, trigger: Trigger, out: &mut Vec<Command>) {
    for rule in rule::load(env.storage) {
        if !rule.on || rule.trigger != trigger {
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

/// Sets the timer to the next time of day a rule waits for.
fn schedule(env: &Env<'_>, out: &mut Vec<Command>) {
    let Some(now) = time::local_time(env) else {
        return;
    };
    let minute_now = u16::from(now.hour) * 60 + u16::from(now.minute);
    let next = rule::load(env.storage)
        .into_iter()
        .filter(|r| r.on)
        .filter_map(|r| match r.trigger {
            Trigger::Time(at) => Some((rule::minutes_until(minute_now, at), at)),
            _ => None,
        })
        .min();
    if let Some((ahead, at)) = next {
        // At the start of that minute.
        let millis = u64::from(ahead) * 60_000 - u64::from(now.second) * 1000;
        out.push(Command::Schedule(
            TIME,
            Params::new().with(MINUTE, at.to_string()),
            env.now.add_millis(millis),
        ));
    }
}

/// When a rule applies, in words.
fn trigger_text(env: &Env<'_>, trigger: Trigger) -> String {
    match trigger {
        Trigger::Start => env.i18n.tr("rules.when.start").to_string(),
        Trigger::Time(minute) => env.i18n.format(
            "rules.when.time",
            &[("time", Arg::Str(&rule::clock(minute)))],
        ),
        Trigger::BookOpened => env.i18n.tr("rules.when.book").to_string(),
        Trigger::Resume => env.i18n.tr("rules.when.resume").to_string(),
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
            let title = env
                .settings
                .specs()
                .find(|s| s.id == *id)
                .map_or(id.as_str(), |s| s.title.as_ref());
            let state = env.i18n.tr(if *on { "rules.on" } else { "rules.off" });
            env.i18n.format(
                "rules.do.switch",
                &[
                    ("setting", Arg::Str(env.i18n.tr(title))),
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
                run(env, Trigger::Start, out);
                schedule(env, out);
            }),
        )?;
        r.subscribe(
            events::BOOK_OPENED,
            Box::new(|env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                run(env, Trigger::BookOpened, out);
            }),
        )?;
        r.subscribe(
            events::RESUME,
            Box::new(|env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                run(env, Trigger::Resume, out);
                // The clock may have moved on while asleep.
                schedule(env, out);
            }),
        )?;
        r.subscribe(
            TIME,
            Box::new(|env: &Env<'_>, params: &Params, out: &mut Vec<Command>| {
                if let Some(minute) = params.get(MINUTE).and_then(|m| m.parse().ok()) {
                    run(env, Trigger::Time(minute), out);
                }
                schedule(env, out);
            }),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Adds `rule` and sets the timer again.
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

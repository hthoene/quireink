//! Battery use: records the battery while the device is awake, with what
//! it did meanwhile (screen refreshes, input, pages turned), and shows how
//! much it uses an hour awake and for 100 pages, and how long it lasts.
//!
//! The record is a log on the device's storage (see its `log` module), kept for
//! analysis on a computer too: where the battery goes is what to make
//! leaner next.

#![no_std]

extern crate alloc;

mod log;
mod view;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{events, groups, menus};
use quireink_sdk::{
    ActionSpec, Command, Env, Id, Manifest, MenuEntry, Params, Plugin, RegisterError, Registrar,
    SDK_VERSION, ScreenSpec, SettingKind, SettingSpec, SettingValue, Version, time,
};

use log::{Kind, Reading};

pub const PLUGIN: Id = Id::new_static("battery");
/// Records the battery's use.
pub const ENABLED: Id = Id::new_static("battery.enabled");
/// Opens the view.
pub const OPEN: Id = Id::new_static("battery.open");
const SCREEN: Id = Id::new_static("battery.screen");

/// What is counted between readings.
#[derive(Debug, Default)]
struct State {
    /// Pages turned since start-up.
    pages: u32,
    /// A time awake was started in the log.
    awake: bool,
    /// When what each plugin took was last noted.
    plugins_noted: Option<u64>,
}

type Shared = Rc<RefCell<State>>;

/// The battery and what was done, now.
fn reading(env: &Env<'_>, state: &State) -> Reading {
    let battery = env.status.battery;
    Reading {
        ms: env.now.millis(),
        millivolts: battery.and_then(|b| b.millivolts),
        percent: battery.map(|b| b.percent),
        charging: battery.is_some_and(|b| b.charging),
        usage: log::Counts::of(env.usage),
        pages: state.pages,
        unix: time::unix_seconds(env),
    }
}

fn dir() -> alloc::string::String {
    Env::plugin_dir(PLUGIN.as_str())
}

/// Appends `text` to the log `name` (with `header` first), starting a new
/// one when it is full.
fn append(env: &Env<'_>, name: &str, old: &str, header: &str, text: &str) {
    let storage = env.storage;
    let path = format!("{}/{name}", dir());
    match storage.size(&path) {
        Ok(size) if size >= log::MAX_BYTES => {
            let _ = storage.rename(&path, &format!("{}/{old}", dir()));
            let _ = storage.append(&path, header.as_bytes());
        }
        Ok(_) => {}
        Err(_) => {
            let _ = storage.append(&path, header.as_bytes());
        }
    }
    let _ = storage.append(&path, text.as_bytes());
}

/// Adds a line to the log, and what each plugin took when due.
fn record(env: &Env<'_>, state: &mut State, kind: Kind) {
    if !env.settings.bool(ENABLED.as_str()) {
        return;
    }
    let now = reading(env, state);
    append(
        env,
        log::FILE,
        log::OLD,
        log::HEADER,
        &log::line(kind, &now),
    );
    let due = state
        .plugins_noted
        .is_none_or(|at| now.ms.saturating_sub(at) >= log::PLUGINS_EVERY_MS);
    if kind == Kind::End || (kind == Kind::Sample && due) {
        state.plugins_noted = Some(now.ms);
        append(
            env,
            log::PLUGINS,
            "plugins.old.csv",
            log::PLUGINS_HEADER,
            &log::plugin_lines(now.ms, &env.usage.plugins),
        );
    }
    // On the card at once: an empty battery ends it all without warning.
    if kind != Kind::Start {
        let _ = env.storage.sync();
    }
    state.awake = kind != Kind::End;
}

/// The battery use plugin.
#[derive(Debug, Default)]
pub struct Battery;

impl Plugin for Battery {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Battery use"),
            description: Cow::Borrowed(
                "Records the battery while the device is awake and shows how long it lasts.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let state: Shared = Rc::new(RefCell::new(State::default()));
        r.add_setting(
            SettingSpec::new(
                ENABLED,
                "battery.setting.enabled",
                SettingKind::Bool,
                SettingValue::Bool(true),
            )
            .group(groups::SYSTEM)
            .description("battery.setting.enabled.help"),
        )?;

        // Recording: a time awake from start-up or waking to sleep.
        for (event, kind) in [
            (events::STARTED, Kind::Start),
            (events::RESUME, Kind::Start),
            (events::SUSPEND, Kind::End),
        ] {
            let s = state.clone();
            r.subscribe(
                event,
                Box::new(move |env, _, _| record(env, &mut s.borrow_mut(), kind)),
            )?;
        }
        let s = state.clone();
        r.subscribe(
            events::BATTERY_MEASURED,
            Box::new(move |env, _, _| {
                let mut state = s.borrow_mut();
                if !state.awake {
                    record(env, &mut state, Kind::Start);
                }
                record(env, &mut state, Kind::Sample);
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            events::PAGE_TURNED,
            Box::new(move |_, _, _| {
                let mut state = s.borrow_mut();
                state.pages = state.pages.saturating_add(1);
            }),
        )?;

        // The view.
        let s = state.clone();
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("battery.title"),
            factory: Box::new(move |env, _| {
                let now = reading(env, &s.borrow());
                Box::new(view::BatteryView::new(env, now, &dir()))
            }),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("battery.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::TOOLS,
            item: OPEN,
            order: 37,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("battery"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

//! The screen lock: a PIN, a pattern or a password asked for when
//! QuireInk starts and when the device wakes up, before anything else can
//! be used. Off by default.
//!
//! - Set up under Settings › System › *Screen lock*: the code in use is
//!   asked for first, then the kind (off, PIN, pattern, password) and the
//!   new code twice. Patterns are offered on touch screens.
//! - The code is kept as a salted hash among the passwords and keys,
//!   never as it is. Like those, it keeps the device from casual use, not
//!   from someone who takes its files.
//! - Locked, the lock screen takes every input: buttons, taps, swipes in
//!   from the edges. A screen that would open above it (a rule, a timer)
//!   closes it, and it opens again at once.
//! - After five wrong tries, the next one waits half a minute.
//! - *Lock now* (`lock.now`) locks at once, from any button, gesture,
//!   menu or rule.

#![no_std]

extern crate alloc;

mod code;
mod screen;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::builtin::{events, groups};
use quireink_sdk::{
    ActionSpec, Command, Env, Id, Manifest, Params, Plugin, RegisterError, Registrar, SDK_VERSION,
    ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("lock");
/// Locks at once.
pub const NOW: Id = Id::new_static("lock.now");
/// The kind of lock: `lock.kind.off`, `…pin`, `…pattern` or `…password`
/// (ids, as choices are, translated to their names).
pub const KIND: Id = Id::new_static("lock.kind");
/// Whether the lock is asked for when the device wakes up.
pub const ON_WAKE: Id = Id::new_static("lock.on-wake");
const SCREEN: Id = Id::new_static("lock.screen");

/// Whether the device is locked: shared by the plugin's handlers and its
/// screens.
pub(crate) type Locked = Rc<Cell<bool>>;

/// The kinds; the setting stores them after [`KIND_PREFIX`].
pub(crate) const KINDS: [&str; 4] = ["off", "pin", "pattern", "password"];
pub(crate) const KIND_PREFIX: &str = "lock.kind.";

/// Opens the lock screen, if a lock is set and it is not open yet.
fn lock(env: &Env<'_>, locked: &Locked, out: &mut Vec<Command>) {
    if locked.get() || code::active(env).is_none() {
        return;
    }
    locked.set(true);
    out.push(Command::OpenScreen(
        SCREEN,
        Params::new().with(screen::MODE, screen::UNLOCK),
    ));
}

/// The screen lock plugin.
#[derive(Debug, Default)]
pub struct Lock;

impl Plugin for Lock {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Screen lock"),
            description: Cow::Borrowed("A PIN, a pattern or a password when the device wakes up."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let locked: Locked = Rc::new(Cell::new(false));
        let shared = locked.clone();
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("lock.setting.kind"),
            factory: Box::new(move |env, params| {
                Box::new(screen::LockScreen::new(env, params, shared.clone()))
            }),
        })?;
        r.add_setting(
            SettingSpec::new(
                KIND,
                "lock.setting.kind",
                SettingKind::Choice(
                    KINDS
                        .iter()
                        .map(|k| Cow::Owned(alloc::format!("{KIND_PREFIX}{k}")))
                        .collect(),
                ),
                SettingValue::Text(Cow::Borrowed("lock.kind.off")),
            )
            .group(groups::SYSTEM)
            .description("lock.setting.kind.help")
            .editor(SCREEN),
        )?;
        r.add_setting(
            SettingSpec::new(
                ON_WAKE,
                "lock.setting.on-wake",
                SettingKind::Bool,
                SettingValue::Bool(true),
            )
            .group(groups::SYSTEM)
            .advanced()
            .description("lock.setting.on-wake.help"),
        )?;
        let now = locked.clone();
        r.add_action(ActionSpec {
            id: NOW,
            title: Cow::Borrowed("lock.now"),
            handler: Some(Box::new(move |env: &Env<'_>, out: &mut Vec<Command>| {
                lock(env, &now, out);
            })),
        })?;
        r.add_icon(NOW, Cow::Borrowed("lock"))?;
        let start = locked.clone();
        r.subscribe(
            events::STARTED,
            Box::new(move |env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                lock(env, &start, out);
            }),
        )?;
        r.subscribe(
            events::RESUME,
            Box::new(move |env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                if env.settings.bool(ON_WAKE.as_str()) {
                    lock(env, &locked, out);
                }
            }),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
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

//! The kernel's own contributions, registered through the regular plugin
//! interface: standard actions, slots, settings, key bindings and the
//! placeholder screen.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, screens, settings, slots};
use quireink_sdk::{
    ActionHandler, ActionSpec, Canvas, Color, Command, Cx, Env, Handled, Id, Manifest, Plugin,
    Priority, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec, SettingKind, SettingSpec,
    SettingValue, SlotSpec,
};

pub(crate) const CORE: Id = Id::new_static("core");

#[derive(Debug)]
pub(crate) struct CorePlugin;

impl Plugin for CorePlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: CORE,
            name: Cow::Borrowed("QuireInk kernel"),
            version: SDK_VERSION,
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let screen_actions = [
            (actions::NEXT_PAGE, "core.action.next-page"),
            (actions::PREV_PAGE, "core.action.prev-page"),
            (actions::UP, "core.action.up"),
            (actions::DOWN, "core.action.down"),
            (actions::LEFT, "core.action.left"),
            (actions::RIGHT, "core.action.right"),
            (actions::CONFIRM, "core.action.confirm"),
            (actions::MENU, "core.action.menu"),
        ];
        for (id, title) in screen_actions {
            r.add_action(ActionSpec {
                id,
                title: Cow::Borrowed(title),
                handler: None,
            })?;
        }

        let handled: [(Id, &'static str, ActionHandler); 3] = [
            (
                actions::BACK,
                "core.action.back",
                Box::new(|_: &Env<'_>, out: &mut Vec<Command>| out.push(Command::Back)),
            ),
            (
                actions::HOME,
                "core.action.home",
                Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                    out.push(Command::OpenSlot(slots::HOME));
                }),
            ),
            (
                actions::TOGGLE_NIGHT_MODE,
                "core.action.toggle-night-mode",
                Box::new(|env: &Env<'_>, out: &mut Vec<Command>| {
                    let night = env.settings.bool(settings::NIGHT_MODE.as_str());
                    out.push(Command::SetSetting(
                        settings::NIGHT_MODE,
                        SettingValue::Bool(!night),
                    ));
                }),
            ),
        ];
        for (id, title, handler) in handled {
            r.add_action(ActionSpec {
                id,
                title: Cow::Borrowed(title),
                handler: Some(handler),
            })?;
        }

        r.add_slot(SlotSpec {
            id: slots::HOME,
            title: Cow::Borrowed("core.slot.home"),
        })?;
        r.add_screen(ScreenSpec {
            id: screens::PLACEHOLDER,
            title: Cow::Borrowed("core.screen.placeholder"),
            factory: Box::new(|_| Box::new(PlaceholderScreen)),
        })?;
        // Lowest possible priority: any real screen wins.
        r.bind_slot(slots::HOME, screens::PLACEHOLDER, Priority(i16::MIN))?;

        r.add_setting(SettingSpec {
            id: settings::LANGUAGE,
            title: Cow::Borrowed("core.setting.language"),
            kind: SettingKind::Text,
            default: SettingValue::Text(Cow::Borrowed(quireink_i18n::FALLBACK_LANGUAGE)),
        })?;
        r.add_setting(SettingSpec {
            id: settings::NIGHT_MODE,
            title: Cow::Borrowed("core.setting.night-mode"),
            kind: SettingKind::Bool,
            default: SettingValue::Bool(false),
        })?;
        r.add_setting(SettingSpec {
            id: settings::FULL_REFRESH_INTERVAL,
            title: Cow::Borrowed("core.setting.full-refresh-interval"),
            kind: SettingKind::Int { min: 0, max: 1000 },
            default: SettingValue::Int(6),
        })?;
        r.add_setting(SettingSpec {
            id: settings::LONG_PRESS_MS,
            title: Cow::Borrowed("core.setting.long-press-ms"),
            kind: SettingKind::Int {
                min: 200,
                max: 3000,
            },
            default: SettingValue::Int(500),
        })?;

        r.add_keymap(Cow::Borrowed(include_str!("../keymaps/default.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Shown when no plugin provides a screen for a slot.
#[derive(Debug)]
struct PlaceholderScreen;

impl Screen for PlaceholderScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let area = canvas.clip();
        let margin = env.metrics.mm(4);
        let line = env.metrics.tenth_mm(5);
        let (w, h) = (u32::from(area.width), u32::from(area.height));
        let inner_w = w.saturating_sub(2 * margin);
        let inner_h = h.saturating_sub(2 * margin);
        canvas.stroke_rect(
            margin as i32,
            margin as i32,
            inner_w,
            inner_h,
            line,
            Color::BLACK,
        );
        // A cross marks the empty slot until text rendering exists.
        let bar = env.metrics.mm(2);
        canvas.fill_rect(
            margin as i32,
            (h / 2 - bar / 2) as i32,
            inner_w,
            bar,
            Color::gray(128),
        );
        canvas.fill_rect(
            (w / 2 - bar / 2) as i32,
            margin as i32,
            bar,
            inner_h,
            Color::gray(128),
        );
    }

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        Handled::No
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

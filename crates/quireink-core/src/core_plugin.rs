//! The kernel's own contributions, registered through the regular plugin
//! interface: standard actions, slots, settings, key bindings and the
//! placeholder screen.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, fonts, screens, settings, slots};
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
                    out.push(Command::open_slot(slots::HOME));
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

        for (id, title) in [
            (slots::HOME, "core.slot.home"),
            (slots::READER, "core.slot.reader"),
            (slots::LIBRARY, "core.slot.library"),
        ] {
            r.add_slot(SlotSpec {
                id: id.clone(),
                title: Cow::Borrowed(title),
            })?;
            // Lowest possible priority: any real screen wins.
            r.bind_slot(id, screens::PLACEHOLDER, Priority(i16::MIN))?;
        }
        r.add_screen(ScreenSpec {
            id: screens::PLACEHOLDER,
            title: Cow::Borrowed("core.screen.placeholder"),
            factory: Box::new(|_, _| Box::new(PlaceholderScreen)),
        })?;

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
        if let Some(font) = env.fonts.role(fonts::UI.as_str()) {
            let size = env.metrics.pt(14.0);
            let text = env.i18n.tr("core.screen.placeholder");
            let width = font.measure(size, text);
            let x = (w as f32 - width) / 2.0;
            let baseline = (h / 2) as i32 + (size / 3.0) as i32;
            font.draw(canvas, size, x, baseline, text, Color::BLACK);
        }
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

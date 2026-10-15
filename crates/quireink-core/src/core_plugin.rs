//! The kernel's own contributions, registered through the regular plugin
//! interface: standard actions, slots, settings, key bindings and the
//! placeholder screen.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;

use quireink_sdk::builtin::{
    actions, fonts, groups, keymap_presets, menus, params, screens, setting_sections, settings,
    settings_sections, slots, status_items,
};
use quireink_sdk::time::{self, AUTO, CLOCK_12H, CLOCK_24H, TIME_ZONES};
use quireink_sdk::{
    ActionHandler, ActionSpec, Canvas, Color, Command, Cx, Env, Handled, Id, Manifest, MenuEntry,
    Params, Plugin, Priority, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingGroupSpec, SettingKind, SettingSpec, SettingValue, SlotSpec, StatusItemSpec,
    StatusValue,
};

pub(crate) const CORE: Id = Id::new_static("core");

#[derive(Debug)]
pub(crate) struct CorePlugin;

impl Plugin for CorePlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: CORE,
            name: Cow::Borrowed("QuireInk kernel"),
            description: Cow::Borrowed("Runs screens, input, settings and plugins."),
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
        ];
        for (id, title) in screen_actions {
            r.add_action(ActionSpec {
                id,
                title: Cow::Borrowed(title),
                handler: None,
            })?;
        }

        let open = |slot: Id| -> ActionHandler {
            Box::new(move |_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::open_slot(slot.clone()));
            })
        };
        let forward = |target: Id| -> ActionHandler {
            Box::new(move |_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::RunAction(target.clone()));
            })
        };
        let handled: [(Id, &'static str, ActionHandler); 10] = [
            // Screens without their own meaning of next/previous turn pages.
            (
                actions::NEXT,
                "core.action.next",
                forward(actions::NEXT_PAGE),
            ),
            (
                actions::PREVIOUS,
                "core.action.previous",
                forward(actions::PREV_PAGE),
            ),
            (
                actions::MENU,
                "core.action.menu",
                Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                    out.push(Command::OpenSlot(
                        slots::MENU,
                        Params::new().with(params::MENU, menus::MAIN.as_str()),
                    ));
                }),
            ),
            (
                actions::LIBRARY,
                "core.action.library",
                open(slots::LIBRARY),
            ),
            (
                actions::SETTINGS,
                "core.action.settings",
                open(slots::SETTINGS),
            ),
            (
                actions::BACK,
                "core.action.back",
                Box::new(|_: &Env<'_>, out: &mut Vec<Command>| out.push(Command::Back)),
            ),
            (
                actions::NOTHING,
                "core.action.nothing",
                Box::new(|_: &Env<'_>, _: &mut Vec<Command>| {}),
            ),
            (
                actions::HOME,
                "core.action.home",
                Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                    out.push(Command::open_slot(slots::HOME));
                }),
            ),
            (
                actions::SLEEP,
                "core.action.sleep",
                Box::new(|env: &Env<'_>, out: &mut Vec<Command>| {
                    // The sleep screen first, if there is one, then sleep.
                    if env.sleep && env.has_slot(&slots::SLEEP) {
                        out.push(Command::open_slot(slots::SLEEP));
                    }
                    out.push(Command::Sleep);
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
            (slots::MENU, "core.slot.menu"),
            (slots::SETTINGS, "core.slot.settings"),
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

        // The main menu in three sections: reading, tools (plugins add
        // theirs between 30 and 79) and the device.
        for (id, title, order) in [
            (
                quireink_sdk::builtin::sections::READING,
                "core.section.reading",
                0,
            ),
            (
                quireink_sdk::builtin::sections::TOOLS,
                "core.section.tools",
                30,
            ),
            (
                quireink_sdk::builtin::sections::SYSTEM,
                "core.section.system",
                80,
            ),
        ] {
            r.add_menu_section(quireink_sdk::MenuSection {
                id,
                title: Cow::Borrowed(title),
                menu: menus::MAIN,
                order,
            })?;
        }
        for (action, order, icon) in [
            (actions::HOME, 10, "home"),
            (actions::LIBRARY, 20, "books"),
            (settings::NIGHT_MODE, 85, "moon"),
            (actions::SETTINGS, 90, "settings"),
        ] {
            r.add_menu_entry(MenuEntry {
                menu: menus::MAIN,
                item: action.clone(),
                order,
            })?;
            r.add_icon(action, Cow::Borrowed(icon))?;
        }

        // The settings' main page: reading, the device, customising.
        for (id, title, order) in [
            (
                settings_sections::READING,
                "core.settings-section.reading",
                0,
            ),
            (
                settings_sections::DEVICE,
                "core.settings-section.device",
                50,
            ),
            (
                settings_sections::CUSTOMISE,
                "core.settings-section.customise",
                200,
            ),
        ] {
            r.add_menu_section(quireink_sdk::MenuSection {
                id,
                title: Cow::Borrowed(title),
                menu: menus::SETTINGS,
                order,
            })?;
        }
        for (id, title, order, icon) in [
            (groups::READING, "core.group.reading", 10, "book"),
            (groups::LIBRARY, "core.group.library", 20, "books"),
            (groups::DISPLAY, "core.group.display", 60, "contrast"),
            (groups::CONTROLS, "core.group.controls", 70, "adjustments"),
            (groups::SYSTEM, "core.group.system", 90, "device-tablet"),
            (groups::LOOK, "core.group.look", 210, "palette"),
            (groups::OTHER, "core.group.other", 100, "dots"),
        ] {
            r.add_setting_group(SettingGroupSpec {
                id: id.clone(),
                title: Cow::Borrowed(title),
                order,
            })?;
            r.add_icon(id, Cow::Borrowed(icon))?;
        }

        r.add_setting(
            SettingSpec::new(
                settings::LANGUAGE,
                "core.setting.language",
                SettingKind::Language,
                SettingValue::Text(Cow::Borrowed(quireink_i18n::FALLBACK_LANGUAGE)),
            )
            .group(groups::SYSTEM)
            .description("core.setting.language.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::NIGHT_MODE,
                "core.setting.night-mode",
                SettingKind::Bool,
                SettingValue::Bool(false),
            )
            .group(groups::DISPLAY)
            .description("core.setting.night-mode.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::UI_SIZE,
                "core.setting.ui-size",
                SettingKind::Int { min: 7, max: 24 },
                SettingValue::Int(11),
            )
            .group(groups::LOOK)
            .unit("core.unit.pt")
            .description("core.setting.ui-size.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::UI_HEADER,
                "core.setting.ui-header",
                SettingKind::Choice(alloc::vec![
                    Cow::Borrowed(settings::HEADER_DARK),
                    Cow::Borrowed(settings::HEADER_LIGHT),
                ]),
                SettingValue::Text(Cow::Borrowed(settings::HEADER_DARK)),
            )
            .group(groups::LOOK)
            .description("core.setting.ui-header.help"),
        )?;
        for (id, title, (min, max, default), unit, step) in [
            (
                settings::UI_MARGIN,
                "core.setting.ui-margin",
                (0, 15, 3),
                "core.unit.mm",
                1,
            ),
            (
                settings::UI_ROW,
                "core.setting.ui-row",
                (120, 300, 180),
                "core.unit.percent",
                10,
            ),
            (
                settings::UI_LINES,
                "core.setting.ui-lines",
                (1, 10, 3),
                "core.unit.tenth-mm",
                1,
            ),
        ] {
            r.add_setting(
                SettingSpec::new(
                    id,
                    title,
                    SettingKind::Int { min, max },
                    SettingValue::Int(default),
                )
                .group(groups::LOOK)
                .unit(unit)
                .step(step)
                .description(match title {
                    "core.setting.ui-margin" => "core.setting.ui-margin.help",
                    "core.setting.ui-row" => "core.setting.ui-row.help",
                    _ => "core.setting.ui-lines.help",
                }),
            )?;
        }
        r.add_setting(
            SettingSpec::new(
                settings::KEYMAP_PRESET,
                "core.setting.keymap-preset",
                // The kernel fills in the registered presets.
                SettingKind::Choice(alloc::vec![Cow::Borrowed(keymap_presets::STANDARD_NAME)]),
                SettingValue::Text(Cow::Borrowed(keymap_presets::STANDARD_NAME)),
            )
            .group(groups::CONTROLS)
            .description("core.setting.keymap-preset.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::FULL_REFRESH_INTERVAL,
                "core.setting.full-refresh-interval",
                SettingKind::Int { min: 0, max: 1000 },
                SettingValue::Int(6),
            )
            .group(groups::DISPLAY)
            .advanced()
            .unit("core.unit.pages")
            .description("core.setting.full-refresh-interval.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::NOTICE_SECONDS,
                "core.setting.notice-seconds",
                SettingKind::Int { min: 1, max: 60 },
                SettingValue::Int(4),
            )
            .group(groups::DISPLAY)
            .advanced()
            .unit("core.unit.seconds")
            .description("core.setting.notice-seconds.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::LONG_PRESS_MS,
                "core.setting.long-press-ms",
                SettingKind::Int {
                    min: 200,
                    max: 3000,
                },
                SettingValue::Int(500),
            )
            .group(groups::CONTROLS)
            .advanced()
            .unit("core.unit.ms")
            .description("core.setting.long-press-ms.help"),
        )?;

        r.add_setting(
            SettingSpec::new(
                settings::READING_FONT,
                "core.setting.reading-font",
                // The kernel fills in the registered families.
                SettingKind::Choice(alloc::vec![Cow::Borrowed(settings::DEFAULT_FONT)]),
                SettingValue::Text(Cow::Borrowed(settings::DEFAULT_FONT)),
            )
            .group(groups::READING)
            .section(setting_sections::FONT)
            .scoped()
            .description("core.setting.reading-font.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::SCOPE_ONLY,
                "core.setting.scope-only",
                SettingKind::Bool,
                SettingValue::Bool(false),
            )
            .group(groups::READING)
            .scoped()
            .description("core.setting.scope-only.help"),
        )?;
        // The time of day and the battery, for status lines.
        r.add_setting(
            SettingSpec::new(
                settings::CLOCK_FORMAT,
                "core.setting.clock-format",
                SettingKind::Choice(alloc::vec![
                    Cow::Borrowed(CLOCK_24H),
                    Cow::Borrowed(CLOCK_12H)
                ]),
                SettingValue::Text(Cow::Borrowed(CLOCK_24H)),
            )
            .group(groups::SYSTEM),
        )?;
        let mut zones = alloc::vec![Cow::Borrowed(AUTO)];
        zones.extend(TIME_ZONES.iter().map(|(id, _)| Cow::Borrowed(*id)));
        r.add_setting(
            SettingSpec::new(
                settings::TIME_ZONE,
                "core.setting.time-zone",
                SettingKind::Choice(zones),
                SettingValue::Text(Cow::Borrowed(AUTO)),
            )
            .group(groups::SYSTEM)
            .description("core.setting.time-zone.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                settings::TIME_ZONE_RULE,
                "core.setting.time-zone-rule",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("")),
            )
            .group(groups::SYSTEM)
            .advanced()
            .description("core.setting.time-zone-rule.help"),
        )?;
        r.add_status_item(StatusItemSpec {
            id: status_items::CLOCK,
            title: Cow::Borrowed("core.status.clock"),
            value: Box::new(|env, _| time::clock_text(env).map(StatusValue::Text)),
        })?;
        r.add_status_item(StatusItemSpec {
            id: status_items::BATTERY,
            title: Cow::Borrowed("core.status.battery"),
            value: Box::new(|env, _| {
                env.status.battery.map(|b| StatusValue::Level {
                    percent: b.percent,
                    charging: b.charging,
                })
            }),
        })?;

        r.add_keymap(Cow::Borrowed(include_str!("../keymaps/default.ini")))?;
        r.add_keymap_preset(
            keymap_presets::TWO_BUTTONS,
            Cow::Borrowed(include_str!("../keymaps/two-buttons.ini")),
        )?;
        r.add_keymap_preset(
            keymap_presets::LEFT_HANDED,
            Cow::Borrowed(include_str!("../keymaps/left-handed.ini")),
        )?;
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

//! The paged reader.
//!
//! - Shows any document a format plugin can open, page by page, in the
//!   reading font role, with configurable size, spacing, margins,
//!   justification and paragraph indents.
//! - Saves the reading position per book and reopens the last book at
//!   start-up (setting `reader.open-last`).
//! - Builds a page index in the background, so page numbers and total pages
//!   become available without slowing down page turns.
//! - Emits the standard events `core.book-opened` and `core.page-turned`,
//!   so other plugins (home screen, statistics, sync, …) follow the reading.
//!
//! It is bound to the `core.reader` slot; any other plugin can replace it.

#![no_std]

extern crate alloc;

mod index;
mod screen;
mod toc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::builtin::{
    actions as core_actions, events, groups, params, settings as core_settings, slots,
};
use quireink_sdk::{
    Command, Env, Id, Manifest, Params, Plugin, Priority, RegisterError, Registrar, SDK_VERSION,
    ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("reader");
pub const SCREEN: Id = Id::new_static("reader.screen");
/// The table of contents screen; parameters `path`, `section`, `offset`.
pub const TOC_SCREEN: Id = Id::new_static("reader.toc");
/// Asks the reader to show a position. Parameters: `path` (the book),
/// `section` and `offset`. Emitted by the table of contents; bookmark or
/// search plugins can use it too.
pub const GOTO: Id = Id::new_static("reader.goto");

/// Settings of the reader.
pub mod settings {
    use quireink_sdk::Id;

    /// Font size in points.
    pub const FONT_SIZE: Id = Id::new_static("reader.font-size");
    /// Distance between baselines in percent of the font size.
    pub const LINE_SPACING: Id = Id::new_static("reader.line-spacing");
    /// Page margins in millimetres.
    pub const MARGIN: Id = Id::new_static("reader.margin");
    pub const JUSTIFY: Id = Id::new_static("reader.justify");
    /// Indent first lines instead of spacing paragraphs apart.
    pub const INDENT: Id = Id::new_static("reader.indent");
    /// Reopen the last book at start-up.
    pub const OPEN_LAST: Id = Id::new_static("reader.open-last");
    /// Hyphenate words at line ends, in the book's language.
    pub const HYPHENATE: Id = Id::new_static("reader.hyphenate");
}

/// The reader's menu (`core.menu` on the reader screen). Any plugin can
/// add entries; users can redefine it in `[menus]`.
pub const MENU: Id = Id::new_static("reader.menu");

/// Actions of the reader.
pub mod actions {
    use quireink_sdk::Id;

    pub const FONT_LARGER: Id = Id::new_static("reader.font-larger");
    pub const FONT_SMALLER: Id = Id::new_static("reader.font-smaller");
    pub const TOGGLE_JUSTIFY: Id = Id::new_static("reader.toggle-justify");
    /// Opens the table of contents.
    pub const TOC: Id = Id::new_static("reader.show-toc");
}

/// The reader plugin.
#[derive(Debug, Default)]
pub struct Reader;

impl Plugin for Reader {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Reader"),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("reader.title"),
            factory: Box::new(|env, params| Box::new(screen::ReaderScreen::open(env, params))),
        })?;
        r.bind_slot(slots::READER, SCREEN, Priority::DEFAULT)?;
        r.add_screen(ScreenSpec {
            id: TOC_SCREEN,
            title: Cow::Borrowed("reader.toc"),
            factory: Box::new(|env, params| Box::new(toc::TocScreen::open(env, params))),
        })?;
        r.add_action(quireink_sdk::ActionSpec {
            id: actions::TOC,
            title: Cow::Borrowed("reader.toc"),
            handler: None,
        })?;

        let int = |id, key: &'static str, (min, max, default), unit: &'static str| {
            SettingSpec::new(
                id,
                key,
                SettingKind::Int { min, max },
                SettingValue::Int(default),
            )
            .group(groups::READING)
            .unit(unit)
        };
        let flag = |id, key: &'static str, default| {
            SettingSpec::new(id, key, SettingKind::Bool, SettingValue::Bool(default))
                .group(groups::READING)
        };
        for setting in [
            int(
                settings::FONT_SIZE,
                "reader.setting.font-size",
                (6, 40, 11),
                "core.unit.pt",
            )
            .description("reader.setting.font-size.help"),
            int(
                settings::LINE_SPACING,
                "reader.setting.line-spacing",
                (100, 250, 135),
                "core.unit.percent",
            )
            .description("reader.setting.line-spacing.help"),
            int(
                settings::MARGIN,
                "reader.setting.margin",
                (0, 30, 4),
                "core.unit.mm",
            )
            .description("reader.setting.margin.help"),
            flag(settings::JUSTIFY, "reader.setting.justify", true)
                .description("reader.setting.justify.help"),
            flag(settings::HYPHENATE, "reader.setting.hyphenate", true)
                .description("reader.setting.hyphenate.help"),
            flag(settings::INDENT, "reader.setting.indent", true)
                .advanced()
                .description("reader.setting.indent.help"),
            flag(settings::OPEN_LAST, "reader.setting.open-last", true)
                .description("reader.setting.open-last.help"),
        ] {
            r.add_setting(setting)?;
        }

        let font_step = |step: i64| -> quireink_sdk::ActionHandler {
            Box::new(move |env: &Env<'_>, out: &mut Vec<Command>| {
                let size = env.settings.int(settings::FONT_SIZE.as_str()) + step;
                out.push(Command::SetSetting(
                    settings::FONT_SIZE,
                    SettingValue::Int(size.clamp(6, 40)),
                ));
            })
        };
        let handlers: [(Id, &'static str, quireink_sdk::ActionHandler); 3] = [
            (
                actions::FONT_LARGER,
                "reader.action.font-larger",
                font_step(1),
            ),
            (
                actions::FONT_SMALLER,
                "reader.action.font-smaller",
                font_step(-1),
            ),
            (
                actions::TOGGLE_JUSTIFY,
                "reader.action.toggle-justify",
                Box::new(|env: &Env<'_>, out: &mut Vec<Command>| {
                    let justify = env.settings.bool(settings::JUSTIFY.as_str());
                    out.push(Command::SetSetting(
                        settings::JUSTIFY,
                        SettingValue::Bool(!justify),
                    ));
                }),
            ),
        ];
        for (id, title, handler) in handlers {
            r.add_action(quireink_sdk::ActionSpec {
                id,
                title: Cow::Borrowed(title),
                handler: Some(handler),
            })?;
        }
        // Settings in the menu show their state and change in place.
        for (item, order) in [
            (actions::TOC, 5),
            (settings::FONT_SIZE, 10),
            (settings::JUSTIFY, 30),
            (core_settings::NIGHT_MODE, 40),
            (core_actions::LIBRARY, 50),
            (core_actions::SETTINGS, 60),
            (core_actions::HOME, 70),
        ] {
            r.add_menu_entry(quireink_sdk::MenuEntry {
                menu: MENU,
                item,
                order,
            })?;
        }

        r.subscribe(events::STARTED, Box::new(reopen_last_book))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Where the reader remembers the last book.
fn state_path() -> String {
    alloc::format!("{}/state.ini", Env::plugin_dir(PLUGIN.as_str()))
}

fn reopen_last_book(env: &Env<'_>, _: &Params, out: &mut Vec<Command>) {
    if !env.settings.bool(settings::OPEN_LAST.as_str()) {
        return;
    }
    let Ok(data) = quireink_sdk::read_file(env.storage, &state_path(), 4096) else {
        return;
    };
    let Ok(text) = core::str::from_utf8(&data) else {
        return;
    };
    let last = quireink_sdk::config::parse(text)
        .filter_map(Result::ok)
        .find(|e| e.section == "reader" && e.key == "last")
        .map(|e| String::from(e.value.as_str()));
    if let Some(path) = last
        && env.storage.size(&path).is_ok()
    {
        out.push(Command::OpenSlot(
            slots::READER,
            Params::new().with(params::PATH, path),
        ));
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

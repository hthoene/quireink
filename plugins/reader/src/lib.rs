//! The paged reader.
//!
//! - Shows any document a format plugin can open, page by page, in the
//!   reading font role, with configurable size, spacing, margins,
//!   justification and paragraph indents.
//! - Saves the reading position per book and reopens the last book at
//!   start-up (setting `reader.open-last`).
//! - Builds a page index in the background, so page numbers and total pages
//!   become available without slowing down page turns.
//! - Emits `reader.book-opened` and `reader.page-turned`, so other plugins
//!   (statistics, sync, …) can follow the reading.
//!
//! It is bound to the `core.reader` slot; any other plugin can replace it.

#![no_std]

extern crate alloc;

mod index;
mod screen;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::builtin::{events, params, slots};
use quireink_sdk::{
    Command, Env, Id, Manifest, Params, Plugin, Priority, RegisterError, Registrar, SDK_VERSION,
    ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("reader");
pub const SCREEN: Id = Id::new_static("reader.screen");

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
}

/// Events emitted by the reader; all carry the parameter `path`.
pub mod events_out {
    use quireink_sdk::Id;

    pub const BOOK_OPENED: Id = Id::new_static("reader.book-opened");
    /// Also carries `progress` (percent with one decimal).
    pub const PAGE_TURNED: Id = Id::new_static("reader.page-turned");
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

        let int = |id, title, min, max, default| SettingSpec {
            id,
            title: Cow::Borrowed(title),
            kind: SettingKind::Int { min, max },
            default: SettingValue::Int(default),
        };
        let flag = |id, title, default| SettingSpec {
            id,
            title: Cow::Borrowed(title),
            kind: SettingKind::Bool,
            default: SettingValue::Bool(default),
        };
        for setting in [
            int(settings::FONT_SIZE, "reader.setting.font-size", 6, 40, 11),
            int(
                settings::LINE_SPACING,
                "reader.setting.line-spacing",
                100,
                250,
                135,
            ),
            int(settings::MARGIN, "reader.setting.margin", 0, 30, 4),
            flag(settings::JUSTIFY, "reader.setting.justify", true),
            flag(settings::INDENT, "reader.setting.indent", true),
            flag(settings::OPEN_LAST, "reader.setting.open-last", true),
        ] {
            r.add_setting(setting)?;
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

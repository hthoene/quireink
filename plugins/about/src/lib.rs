//! *About QuireInk*, in the device's settings: its version, its licence
//! and the licences of what it bundles (fonts, icons, hyphenation
//! patterns), which every copy of it must carry. Shown by the reader (as
//! Markdown), so it pages and reads like a book.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;

use quireink_sdk::builtin::{menus, params, slots};
use quireink_sdk::{
    ActionSpec, Command, Env, Id, Manifest, MenuEntry, Params, Plugin, RegisterError, Registrar,
    SDK_VERSION, Version,
};

pub const PLUGIN: Id = Id::new_static("about");
/// Shows the page.
pub const OPEN: Id = Id::new_static("about.open");
/// The licences, from `tools/licences/build.py`.
const LICENCES: &str = include_str!("../licences.md");
/// Where the page is written for the reader: a hidden folder, so it is
/// no book.
const PAGE: &str = ".quireink/about.md";

/// The page's text.
fn page(env: &Env<'_>) -> alloc::string::String {
    let intro = env.i18n.tr("about.intro");
    format!(
        "# QuireInk {}\n\n{intro}\n\nhttps://github.com/hthoene/quireink\n\n{LICENCES}",
        env!("CARGO_PKG_VERSION")
    )
}

/// The about page.
#[derive(Debug, Default)]
pub struct About;

impl Plugin for About {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("About"),
            description: Cow::Borrowed("QuireInk's version and licences."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("about.title"),
            handler: Some(Box::new(|env: &Env<'_>, out: &mut Vec<Command>| {
                let _ = env.storage.create_dir(".quireink");
                if env.storage.write(PAGE, page(env).as_bytes()).is_ok() {
                    out.push(Command::OpenSlot(
                        slots::READER,
                        Params::new().with(params::PATH, PAGE),
                    ));
                }
            })),
        })?;
        // In the device's part of the settings, last.
        r.add_menu_entry(MenuEntry {
            menu: menus::SETTINGS,
            item: OPEN,
            order: 99,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("info-circle"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use quireink_i18n::{Catalog, check};

    use super::LICENCES;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// Words only, so lines joined into paragraphs compare equal.
    fn words(text: &str) -> std::string::String {
        text.split_whitespace()
            .collect::<std::vec::Vec<_>>()
            .join(" ")
    }

    #[test]
    fn every_licence_is_there() {
        // As they are in the repository: run tools/licences/build.py
        // after one changed.
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
        for file in [
            "NOTICE",
            "LICENSE",
            "plugins/fonts-default/fonts/Literata-OFL.txt",
            "plugins/fonts-default/fonts/AtkinsonHyperlegibleNext-OFL.txt",
            "plugins/fonts-default/fonts/TablerIcons-MIT.txt",
        ] {
            let Ok(text) = std::fs::read_to_string(std::format!("{root}{file}")) else {
                continue;
            };
            assert!(
                words(LICENCES).contains(&words(&text)),
                "{file} changed: run tools/licences/build.py"
            );
        }
        assert!(LICENCES.contains("Gerard D.C. Kuiken"));
        assert!(LICENCES.contains("Stephan Hennig"));
    }
}

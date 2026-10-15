//! The home screen's pages: each a list of widgets (ids separated by
//! commas) in a setting of its own, `home.page-1` to `home.page-4`. The
//! first page always shows; the others where they have widgets.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::groups;
use quireink_sdk::status::item_ids;
use quireink_sdk::{
    Command, Env, Id, RegisterError, Registrar, SettingKind, SettingSpec, SettingValue,
};

use crate::EDIT_SCREEN;

/// Pages at most.
pub(crate) const MAX_PAGES: usize = 4;
const SECTION: &str = "home.section";

/// The built-in widgets (see `blocks`).
pub(crate) const STATUS: &str = "home.status";
pub(crate) const DATE: &str = "home.date";
pub(crate) const CONTINUE: &str = "home.continue";
pub(crate) const RECENT: &str = "home.recent";
pub(crate) const TOOLS: &str = "home.tools";

/// What the pages show at first: the book being read and the recent ones,
/// then the tools.
const DEFAULTS: [&str; MAX_PAGES] = [
    "home.status, home.date, home.continue, home.recent",
    "home.status, home.tools",
    "",
    "",
];

/// The setting of page `index` (0 the first).
pub(crate) fn setting(index: usize) -> Id {
    Id::parse(&format!("home.page-{}", index + 1)).unwrap_or(Id::new_static("home.page-1"))
}

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    for (index, default) in DEFAULTS.iter().enumerate() {
        let mut spec = SettingSpec::new(
            setting(index),
            format!("home.setting.page-{}", index + 1),
            SettingKind::List,
            SettingValue::Text(Cow::Borrowed(default)),
        )
        .group(groups::LOOK)
        .section(SECTION)
        .editor(EDIT_SCREEN)
        .description("home.setting.page.help");
        if index >= 2 {
            spec = spec.advanced();
        }
        r.add_setting(spec)?;
    }
    Ok(())
}

/// The pages shown: their widgets' ids, the first page always.
pub(crate) fn pages(env: &Env<'_>) -> Vec<Vec<String>> {
    let mut pages: Vec<Vec<String>> = (0..MAX_PAGES)
        .map(|index| {
            item_ids(env.settings.text(setting(index).as_str()))
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .enumerate()
        .filter(|(index, page)| *index == 0 || !page.is_empty())
        .map(|(_, page)| page)
        .collect();
    if pages.is_empty() {
        pages.push(Vec::new());
    }
    pages
}

/// The commands that store `pages` (the rest emptied).
pub(crate) fn store(pages: &[Vec<String>]) -> Vec<Command> {
    (0..MAX_PAGES)
        .map(|index| {
            let text = pages.get(index).map(|p| p.join(", ")).unwrap_or_default();
            Command::SetSetting(setting(index), SettingValue::Text(Cow::Owned(text)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_is_stored() {
        let pages = alloc::vec![alloc::vec![String::from("a"), String::from("b")]];
        assert_eq!(store(&pages).len(), MAX_PAGES);
    }
}

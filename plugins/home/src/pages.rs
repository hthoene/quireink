//! The home screen's pages: each a grid of items in a setting of its own,
//! `home.page-1` to `home.page-4` (see `grid`). The first page always
//! shows; the others where they have items. Above the grid sits the
//! status line, below it the page dots and the pencil.

use alloc::borrow::Cow;
use alloc::format;
use alloc::vec::Vec;

use quireink_sdk::builtin::groups;
use quireink_sdk::widgets;
use quireink_sdk::{
    Command, Env, Id, RegisterError, Registrar, SettingKind, SettingSpec, SettingValue,
};

use crate::SCREEN;
use crate::grid::{self, Item, What};

/// Pages at most.
pub(crate) const MAX_PAGES: usize = 4;
const SECTION: &str = "home.section";

/// Columns of the grid.
pub(crate) const COLUMNS: Id = Id::new_static("home.columns");
/// Rows of the grid; 0: as many as make cells about as high as wide.
pub(crate) const ROWS: Id = Id::new_static("home.rows");
/// Whether the status line shows above the grid.
pub(crate) const STATUS_BAR: Id = Id::new_static("home.status-bar");

/// The built-in widgets (see `blocks`).
pub(crate) const STATUS: &str = "home.status";
pub(crate) const DATE: &str = "home.date";
pub(crate) const CONTINUE: &str = "home.continue";
pub(crate) const RECENT: &str = "home.recent";
/// On a page: every tool not on it otherwise, each in a cell.
pub(crate) const TOOLS: &str = "home.tools";

/// What the pages show at first: the clock, the book being read and the
/// recent ones, then the tools.
const DEFAULTS: [&str; MAX_PAGES] = [
    "home.clock, home.continue, home.recent",
    "home.tools",
    "",
    "",
];

/// The setting of page `index` (0 the first).
pub(crate) fn setting(index: usize) -> Id {
    Id::parse(&format!("home.page-{}", index + 1)).unwrap_or(Id::new_static("home.page-1"))
}

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    for (index, default) in DEFAULTS.iter().enumerate() {
        r.add_setting(
            SettingSpec::new(
                setting(index),
                format!("home.setting.page-{}", index + 1),
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed(default)),
            )
            .group(groups::LOOK)
            .section(SECTION)
            .editor(SCREEN)
            .description("home.setting.page.help")
            .advanced(),
        )?;
    }
    r.add_setting(
        SettingSpec::new(
            COLUMNS,
            "home.setting.columns",
            SettingKind::Int {
                min: 2,
                max: i64::from(grid::MAX_COLUMNS),
            },
            SettingValue::Int(4),
        )
        .group(groups::LOOK)
        .section(SECTION)
        .description("home.setting.columns.help"),
    )?;
    r.add_setting(
        SettingSpec::new(
            ROWS,
            "home.setting.rows",
            SettingKind::Int {
                min: 0,
                max: i64::from(grid::MAX_ROWS),
            },
            SettingValue::Int(0),
        )
        .group(groups::LOOK)
        .section(SECTION)
        .description("home.setting.rows.help"),
    )?;
    r.add_setting(
        SettingSpec::new(
            Id::new_static(crate::blocks::TOOL_LABELS),
            "home.setting.tool-labels",
            SettingKind::Bool,
            SettingValue::Bool(true),
        )
        .group(groups::LOOK)
        .section(SECTION)
        .description("home.setting.tool-labels.help"),
    )?;
    r.add_setting(
        SettingSpec::new(
            STATUS_BAR,
            "home.setting.status-bar",
            SettingKind::Bool,
            SettingValue::Bool(true),
        )
        .group(groups::LOOK)
        .section(SECTION)
        .description("home.setting.status-bar.help"),
    )?;
    Ok(())
}

/// The items of every page shown, the first page always.
pub(crate) fn pages(env: &Env<'_>) -> Vec<Vec<Item>> {
    let mut pages: Vec<Vec<Item>> = (0..MAX_PAGES)
        .map(|index| grid::parse(env.settings.text(setting(index).as_str())))
        .enumerate()
        .filter(|(index, page)| *index == 0 || !page.is_empty())
        .map(|(_, page)| page)
        .collect();
    if pages.is_empty() {
        pages.push(Vec::new());
    }
    pages
}

/// A page's items as the grid shows them: the tools for `home.tools`,
/// without what the grid has no use for (the status line above it,
/// spaces: empty cells are).
pub(crate) fn expand(env: &Env<'_>, items: &[Item]) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for item in items {
        match &item.what {
            What::Widget(id) if id == TOOLS => {
                for (action, _) in grid::tools(env) {
                    let shown = items.iter().any(|i| i.what == What::Tool(action.clone()));
                    if !shown {
                        out.push(Item {
                            what: What::Tool(action),
                            at: None,
                        });
                    }
                }
            }
            What::Widget(id)
                if id == widgets::SPACE
                    || id == widgets::FILL
                    || (id == STATUS && env.settings.bool(STATUS_BAR.as_str())) => {}
            What::Widget(id) if !env.widgets.has(id) => {}
            _ => out.push(item.clone()),
        }
    }
    out
}

/// The commands that store `pages` (the rest emptied).
pub(crate) fn store(pages: &[Vec<Item>]) -> Vec<Command> {
    (0..MAX_PAGES)
        .map(|index| {
            let text = pages.get(index).map(|p| grid::write(p)).unwrap_or_default();
            Command::SetSetting(setting(index), SettingValue::Text(Cow::Owned(text)))
        })
        .collect()
}

/// The page whose setting is `id`, if it is one.
pub(crate) fn page_of(id: &str) -> Option<usize> {
    (0..MAX_PAGES).find(|&i| setting(i).as_str() == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_is_stored() {
        let pages = alloc::vec![grid::parse("a 0/0 1x1")];
        assert_eq!(store(&pages).len(), MAX_PAGES);
        assert_eq!(page_of("home.page-2"), Some(1));
        assert_eq!(page_of("home.columns"), None);
    }
}

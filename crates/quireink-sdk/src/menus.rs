use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::id::Id;

/// Puts an action into a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// The menu, e.g. [`builtin::menus::MAIN`](crate::builtin::menus::MAIN)
    /// or `reader.menu`.
    pub menu: Id,
    /// The action run when the entry is chosen; its title is the label.
    pub action: Id,
    /// Position; lower comes first.
    pub order: i16,
}

/// One entry of a resolved menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem<'a> {
    pub action: &'a Id,
    /// Translation key of the label.
    pub title: &'a str,
}

/// All menus: contributed entries, the user's menu definitions and the
/// action titles used as labels.
#[derive(Debug, Default)]
pub struct Menus {
    entries: Vec<MenuEntry>,
    /// Menus redefined by the user (`[menus] reader.menu = a, b, c`).
    custom: BTreeMap<Id, Vec<Id>>,
    titles: BTreeMap<Id, Cow<'static, str>>,
}

impl Menus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, entry: MenuEntry) {
        self.entries.push(entry);
    }

    /// Replaces a menu's entries with exactly `actions`, in this order.
    pub fn customize(&mut self, menu: Id, actions: Vec<Id>) {
        self.custom.insert(menu, actions);
    }

    /// Registers the label of an action.
    pub fn set_title(&mut self, action: Id, title: Cow<'static, str>) {
        self.titles.insert(action, title);
    }

    /// The entries of `menu`, in display order. Actions without a known
    /// title are skipped.
    pub fn items(&self, menu: &str) -> Vec<MenuItem<'_>> {
        if let Some(actions) = self.custom.get(menu) {
            return actions.iter().filter_map(|a| self.item(a)).collect();
        }
        let mut entries: Vec<&MenuEntry> = self
            .entries
            .iter()
            .filter(|e| e.menu.as_str() == menu)
            .collect();
        entries.sort_by_key(|e| e.order);
        entries
            .into_iter()
            .filter_map(|e| self.item(&e.action))
            .collect()
    }

    fn item<'a>(&'a self, action: &'a Id) -> Option<MenuItem<'a>> {
        self.titles.get(action).map(|title| MenuItem {
            action,
            title: title.as_ref(),
        })
    }

    /// Every action referenced by a menu, for validation.
    pub fn referenced_actions(&self) -> impl Iterator<Item = &Id> {
        self.entries
            .iter()
            .map(|e| &e.action)
            .chain(self.custom.values().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn orders_entries_and_honours_user_menus() {
        let mut menus = Menus::new();
        for (action, order) in [("a.second", 20), ("a.first", 10), ("a.untitled", 5)] {
            menus.add(MenuEntry {
                menu: Id::new_static("a.menu"),
                action: Id::parse(action).unwrap(),
                order,
            });
        }
        menus.set_title(Id::new_static("a.first"), Cow::Borrowed("First"));
        menus.set_title(Id::new_static("a.second"), Cow::Borrowed("Second"));
        fn titles(menus: &Menus) -> Vec<&str> {
            menus.items("a.menu").iter().map(|i| i.title).collect()
        }
        assert_eq!(titles(&menus), ["First", "Second"]);

        menus.customize(Id::new_static("a.menu"), vec![Id::new_static("a.second")]);
        assert_eq!(titles(&menus), ["Second"]);
        assert!(menus.items("other").is_empty());
    }
}

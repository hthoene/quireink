use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::collections::VecMap;
use crate::id::Id;

/// Puts an action or a setting into a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// The menu, e.g. [`builtin::menus::MAIN`](crate::builtin::menus::MAIN)
    /// or `reader.menu`.
    pub menu: Id,
    /// An action, run when the entry is chosen, or a setting, shown with
    /// its current value and changed in place. Its title is the label. An
    /// action named like a menu with entries is shown as that submenu. An
    /// action named like a menu with entries is shown as that submenu.
    pub item: Id,
    /// Position; lower comes first.
    pub order: i16,
}

/// One entry of a resolved menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem<'a> {
    /// An action or a setting.
    pub id: &'a Id,
    /// Translation key of the label.
    pub title: &'a str,
}

/// All menus: contributed entries, the user's menu definitions and the
/// action titles used as labels.
#[derive(Debug, Default)]
pub struct Menus {
    entries: Vec<MenuEntry>,
    /// Menus redefined by the user (`[menus] reader.menu = a, b, c`).
    custom: VecMap<Id, Vec<Id>>,
    titles: VecMap<Id, Cow<'static, str>>,
}

impl Menus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Releases unused capacity once registration is complete.
    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_to_fit();
        self.custom.shrink_to_fit();
        for menu in self.custom.values_mut() {
            menu.shrink_to_fit();
        }
        self.titles.shrink_to_fit();
    }

    pub fn add(&mut self, entry: MenuEntry) {
        self.entries.push(entry);
    }

    /// Replaces a menu's entries with exactly `actions`, in this order.
    pub fn customize(&mut self, menu: Id, actions: Vec<Id>) {
        self.custom.insert(menu, actions);
    }

    /// Returns a menu to the entries plugins contribute.
    pub fn reset(&mut self, menu: &str) {
        self.custom.remove(menu);
    }

    /// Whether the user redefined `menu`.
    pub fn is_customized(&self, menu: &str) -> bool {
        self.custom.contains_key(menu)
    }

    /// Every menu: those plugins contribute to and those the user defined.
    pub fn menu_ids(&self) -> Vec<&Id> {
        let mut ids: Vec<&Id> = self
            .entries
            .iter()
            .map(|e| &e.menu)
            .chain(self.custom.keys())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// The ids of `menu`'s entries as plugins contribute them, ignoring
    /// the user's definition.
    pub fn default_ids(&self, menu: &str) -> Vec<&Id> {
        let mut entries: Vec<&MenuEntry> = self
            .entries
            .iter()
            .filter(|e| e.menu.as_str() == menu)
            .collect();
        entries.sort_by_key(|e| e.order);
        entries.into_iter().map(|e| &e.item).collect()
    }

    /// The ids of `menu`'s entries as shown, including entries without a
    /// label.
    pub fn ids(&self, menu: &str) -> Vec<&Id> {
        match self.custom.get(menu) {
            Some(ids) => ids.iter().collect(),
            None => self.default_ids(menu),
        }
    }

    /// Registers the label of an action or setting.
    pub fn set_title(&mut self, item: Id, title: Cow<'static, str>) {
        self.titles.insert(item, title);
    }

    /// The entries of `menu`, in display order. Items without a known
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
            .filter_map(|e| self.item(&e.item))
            .collect()
    }

    fn item<'a>(&'a self, id: &'a Id) -> Option<MenuItem<'a>> {
        self.titles.get(id).map(|title| MenuItem {
            id,
            title: title.as_ref(),
        })
    }

    /// Every action or setting referenced by a menu, for validation.
    pub fn referenced_items(&self) -> impl Iterator<Item = &Id> {
        self.entries
            .iter()
            .map(|e| &e.item)
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
                item: Id::parse(action).unwrap(),
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

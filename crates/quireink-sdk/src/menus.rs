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
    /// action named like a menu with entries is shown as that submenu.
    pub item: Id,
    /// Position; lower comes first.
    pub order: i16,
}

/// A heading in a menu: the entries after it, up to the next heading,
/// belong to it (see [`Registrar::add_menu_section`](crate::Registrar::add_menu_section)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSection {
    /// `<plugin>.<name>`; also usable in the user's menu definitions.
    pub id: Id,
    /// Translation key of the heading.
    pub title: Cow<'static, str>,
    pub menu: Id,
    /// Position among the menu's entries; lower comes first.
    pub order: i16,
}

/// What a menu entry needs to be offered (see
/// [`Registrar::set_menu_need`](crate::Registrar::set_menu_need)), so
/// menus stay short on every device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MenuNeed {
    /// A device that speaks ([`Env::speech`](crate::Env)): hidden
    /// elsewhere, also where the user listed it.
    Speech,
    /// A device where QuireInk shows the sleep screen
    /// ([`Env::sleep`](crate::Env)), hidden elsewhere.
    Sleep,
    /// A device whose screen QuireInk can turn
    /// ([`settings::ROTATION`](crate::builtin::settings::ROTATION)),
    /// hidden elsewhere.
    Rotation,
    /// A device without a touch screen, where it is the way to do what a
    /// tap or a long press does elsewhere (e.g. choosing a link). On touch
    /// devices it is left out of the menus plugins define; users can still
    /// list it.
    NoTouch,
    /// A device with buttons in a row below the screen, whose row of
    /// hints shows what they do
    /// ([`settings::BUTTON_HINTS`](crate::builtin::settings::BUTTON_HINTS)),
    /// hidden elsewhere.
    ButtonRow,
    /// A device whose shell shows the screen in other colours than black
    /// and white ([`settings::TEXT_COLOR`](crate::builtin::settings::TEXT_COLOR)),
    /// hidden elsewhere.
    Colors,
}

/// One entry of a resolved menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem<'a> {
    /// An action or a setting, or a section.
    pub id: &'a Id,
    /// Translation key of the label.
    pub title: &'a str,
    /// The icon's name ([`icons`](crate::icons)), if it has one.
    pub icon: Option<&'a str>,
    /// A heading rather than an entry ([`MenuSection`]).
    pub section: bool,
}

/// All menus: contributed entries, the user's menu definitions and the
/// action titles used as labels.
#[derive(Debug, Default)]
pub struct Menus {
    entries: Vec<MenuEntry>,
    /// Menus redefined by the user (`[menus] reader.menu = a, b, c`).
    custom: VecMap<Id, Vec<Id>>,
    titles: VecMap<Id, Cow<'static, str>>,
    icons: VecMap<Id, Cow<'static, str>>,
    sections: Vec<Id>,
    needs: VecMap<Id, MenuNeed>,
    /// The device has a touch screen; it cannot speak; QuireInk shows
    /// its sleep screen.
    touch: bool,
    mute: bool,
    sleeps: bool,
    /// The screen can be turned.
    turns: bool,
    /// The shell tints the screen.
    tints: bool,
    /// Buttons lie in a row below the screen.
    button_row: bool,
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
        self.icons.shrink_to_fit();
        self.sections.shrink_to_fit();
        self.needs.shrink_to_fit();
    }

    /// What the device offers, for entries with a [`MenuNeed`].
    pub fn set_device(&mut self, touch: bool, speech: bool) {
        self.touch = touch;
        self.mute = !speech;
    }

    /// Whether QuireInk shows the sleep screen here, for entries that
    /// need it ([`MenuNeed::Sleep`]).
    pub fn set_sleep(&mut self, sleeps: bool) {
        self.sleeps = sleeps;
    }

    /// Whether the screen can be turned here, for entries that need it
    /// ([`MenuNeed::Rotation`]).
    pub fn set_rotation(&mut self, turns: bool) {
        self.turns = turns;
    }

    /// Whether buttons lie in a row below the screen, for entries that
    /// need it ([`MenuNeed::ButtonRow`]).
    pub fn set_button_row(&mut self, row: bool) {
        self.button_row = row;
    }

    /// Whether the shell shows the screen in the colours of the settings
    /// [`TEXT_COLOR`](crate::builtin::settings::TEXT_COLOR) and
    /// [`PAPER_COLOR`](crate::builtin::settings::PAPER_COLOR), for entries
    /// that need it ([`MenuNeed::Colors`]).
    pub fn set_colors(&mut self, tints: bool) {
        self.tints = tints;
    }

    /// Offers `item` only where `need` is met.
    pub fn set_need(&mut self, item: Id, need: MenuNeed) {
        self.needs.insert(item, need);
    }

    /// Whether `item` is left out by the menu parameter
    /// [`HIDE`](crate::builtin::params::HIDE) of `params`.
    pub fn hidden(params: &crate::Params, item: &str) -> bool {
        params
            .get(crate::builtin::params::HIDE)
            .is_some_and(|hide| hide.split(',').any(|h| h.trim() == item))
    }

    /// Whether `item` is offered on this device by default, e.g. whether
    /// the settings screen lists a setting.
    pub fn available(&self, item: &str) -> bool {
        self.offered(item, false)
    }

    /// Whether `item` is offered here; `listed` if the user put it into
    /// the menu.
    fn offered(&self, item: &str, listed: bool) -> bool {
        match self.needs.get(item) {
            Some(MenuNeed::Speech) => !self.mute,
            Some(MenuNeed::Sleep) => self.sleeps,
            Some(MenuNeed::Rotation) => self.turns,
            Some(MenuNeed::Colors) => self.tints,
            Some(MenuNeed::ButtonRow) => self.button_row,
            Some(MenuNeed::NoTouch) => listed || !self.touch,
            None => true,
        }
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
            .filter(|e| e.menu.as_str() == menu && self.offered(e.item.as_str(), false))
            .collect();
        entries.sort_by_key(|e| e.order);
        entries.into_iter().map(|e| &e.item).collect()
    }

    /// The ids of `menu`'s entries as shown, including entries without a
    /// label.
    pub fn ids(&self, menu: &str) -> Vec<&Id> {
        match self.custom.get(menu) {
            Some(ids) => ids
                .iter()
                .filter(|id| self.offered(id.as_str(), true))
                .collect(),
            None => self.default_ids(menu),
        }
    }

    /// Registers the label of an action or setting.
    pub fn set_title(&mut self, item: Id, title: Cow<'static, str>) {
        self.titles.insert(item, title);
    }

    /// Gives an action or setting an icon (by name; see
    /// [`icons`](crate::icons)), wherever it is shown.
    pub fn set_icon(&mut self, item: Id, icon: Cow<'static, str>) {
        self.icons.insert(item, icon);
    }

    /// The icon of an action or setting, if it has one.
    pub fn icon(&self, item: &str) -> Option<&str> {
        self.icons.get(item).map(AsRef::as_ref)
    }

    /// Adds a heading to a menu.
    pub fn add_section(&mut self, section: MenuSection) {
        self.titles.insert(section.id.clone(), section.title);
        self.sections.push(section.id.clone());
        self.entries.push(MenuEntry {
            menu: section.menu,
            item: section.id,
            order: section.order,
        });
    }

    /// The position of `item` in `menu` as plugins contribute it, e.g. to
    /// place it among other things listed with the menu's entries.
    pub fn order_of(&self, menu: &str, item: &str) -> Option<i16> {
        self.entries
            .iter()
            .find(|e| e.menu.as_str() == menu && e.item.as_str() == item)
            .map(|e| e.order)
    }

    /// Whether `item` is a heading.
    pub fn is_section(&self, item: &str) -> bool {
        self.sections.iter().any(|s| s.as_str() == item)
    }

    /// The entries of `menu`, in display order. Items without a known
    /// title are skipped.
    pub fn items(&self, menu: &str) -> Vec<MenuItem<'_>> {
        self.ids(menu)
            .into_iter()
            .filter_map(|id| self.item(id))
            .collect()
    }

    fn item<'a>(&'a self, id: &'a Id) -> Option<MenuItem<'a>> {
        self.titles.get(id).map(|title| MenuItem {
            id,
            title: title.as_ref(),
            icon: self.icon(id.as_str()),
            section: self.is_section(id.as_str()),
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

        // Entries for devices without touch, or that speak.
        menus.reset("a.menu");
        menus.add(MenuEntry {
            menu: Id::new_static("a.menu"),
            item: Id::new_static("a.links"),
            order: 30,
        });
        menus.set_title(Id::new_static("a.links"), Cow::Borrowed("Links"));
        menus.set_need(Id::new_static("a.links"), MenuNeed::NoTouch);
        assert_eq!(titles(&menus).last(), Some(&"Links"));
        menus.set_device(true, false);
        assert_eq!(titles(&menus).last(), Some(&"Second"));
        // Listed by the user, it shows; one that needs speech does not.
        menus.set_need(Id::new_static("a.second"), MenuNeed::Speech);
        menus.customize(
            Id::new_static("a.menu"),
            alloc::vec![Id::new_static("a.links"), Id::new_static("a.second")],
        );
        assert_eq!(titles(&menus), ["Links"]);

        // Left out where a menu is opened.
        let params = crate::Params::new().with(crate::builtin::params::HIDE, "a.first, a.links");
        assert!(Menus::hidden(&params, "a.links"));
        assert!(!Menus::hidden(&params, "a.second"));
        assert!(!Menus::hidden(&crate::Params::new(), "a.links"));
    }
}

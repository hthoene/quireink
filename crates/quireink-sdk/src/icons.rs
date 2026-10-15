//! Icons by name, e.g. `books` or `settings`, drawn from the icon font
//! bound to [`fonts::ICONS`](crate::builtin::fonts::ICONS).
//!
//! The names are those of [Tabler Icons](https://tabler.io/icons), of
//! which QuireInk ships the ones it uses (see `tools/icons/subset.py`). An
//! icon theme is a font with other drawings at the same code points, bound
//! to the role in place of the default.
//!
//! Plugins give their menu entries icons by name
//! ([`Registrar::add_icon`](crate::Registrar::add_icon)); a name the font
//! does not know draws nothing.

include!("icon_table.rs");

/// The character of icon `name` in the icon font.
pub fn char(name: &str) -> Option<char> {
    ICONS
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|at| ICONS[at].1)
}

/// The names of all icons, sorted.
pub fn names() -> impl Iterator<Item = &'static str> {
    ICONS.iter().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_found_by_name() {
        assert!(ICONS.windows(2).all(|w| w[0].0 < w[1].0), "sorted");
        assert_eq!(char("home"), Some('\u{eac1}'));
        assert!(char("books").is_some());
        assert_eq!(char("no-such-icon"), None);
        assert!(names().count() > 50);
    }
}

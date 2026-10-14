//! Lean message catalogs for QuireInk.
//!
//! Translations are plain files in the QuireInk config format (see
//! `quireink-config`), one per language and plugin, e.g. `i18n/de.ini`:
//!
//! ```ini
//! [meta]
//! language = de
//! name = Deutsch
//! plural = one-other
//!
//! [messages]
//! home.continue = Weiterlesen
//! library.books.one = {count} Buch
//! library.books.other = {count} Bücher
//! ```
//!
//! Adding a language means adding files — no code changes, unless the
//! language needs a plural rule that [`PluralRule`] does not know yet.
//! Every plugin ships its own catalogs with keys in its namespace; language
//! packs may add catalogs for many plugins at once.
//!
//! A [`Translator`] looks messages up through a fallback chain (for example
//! `de-AT` → `de` → `en`) and finally returns the key itself, so a missing
//! translation never breaks the UI.

#![no_std]

extern crate alloc;

mod catalog;
pub mod check;
mod format;
mod plural;

pub use catalog::{Catalog, CatalogError};
pub use format::Arg;
pub use plural::{PluralCategory, PluralRule};

use alloc::string::String;
use alloc::vec::Vec;

/// The language every message must exist in; the last fallback.
pub const FALLBACK_LANGUAGE: &str = "en";

/// Looks up messages in all loaded catalogs, following the fallback chain of
/// the selected language.
#[derive(Debug, Default)]
pub struct Translator {
    catalogs: Vec<Catalog>,
    language: String,
    /// Catalog indices in lookup order.
    order: Vec<u16>,
}

impl Translator {
    pub fn new() -> Self {
        let mut translator = Self::default();
        translator.set_language(FALLBACK_LANGUAGE);
        translator
    }

    /// Adds a catalog. For the same language, catalogs added later take
    /// precedence, so user overrides can be loaded last.
    pub fn add(&mut self, catalog: Catalog) {
        self.catalogs.push(catalog);
        self.rebuild_order();
    }

    /// Releases unused capacity once all catalogs are added.
    pub fn shrink_to_fit(&mut self) {
        self.catalogs.shrink_to_fit();
    }

    /// Selects the UI language, e.g. `de` or `pt-BR`.
    pub fn set_language(&mut self, language: &str) {
        self.language.clear();
        self.language.push_str(language);
        self.rebuild_order();
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    /// Available languages as `(tag, name)`, each listed once.
    pub fn languages(&self) -> Vec<(&str, &str)> {
        let mut languages: Vec<(&str, &str)> = Vec::new();
        for catalog in &self.catalogs {
            if !languages.iter().any(|(tag, _)| *tag == catalog.language()) {
                languages.push((catalog.language(), catalog.name()));
            }
        }
        languages.sort_unstable();
        languages
    }

    /// The message for `key`, or `key` itself if no catalog has it.
    pub fn tr<'a>(&'a self, key: &'a str) -> &'a str {
        self.ordered()
            .find_map(|catalog| catalog.get(key))
            .unwrap_or(key)
    }

    /// The message for `key` with `{placeholders}` replaced by `args`.
    pub fn format(&self, key: &str, args: &[(&str, Arg<'_>)]) -> String {
        format::interpolate(self.tr(key), args)
    }

    /// Selects the plural form of `key` for `count`, then formats it.
    ///
    /// Looks up `key.<category>` (e.g. `library.books.few`) using each
    /// catalog's own plural rule, falling back to `key.other`. `{count}` is
    /// always available as an argument.
    pub fn plural(&self, key: &str, count: i64, args: &[(&str, Arg<'_>)]) -> String {
        let mut candidate = String::with_capacity(key.len() + 6);
        for catalog in self.ordered() {
            for category in [catalog.plural_rule().category(count), PluralCategory::Other] {
                candidate.clear();
                candidate.push_str(key);
                candidate.push('.');
                candidate.push_str(category.id());
                if let Some(message) = catalog.get(&candidate) {
                    return format::interpolate_with_count(message, count, args);
                }
            }
        }
        String::from(key)
    }

    fn ordered(&self) -> impl Iterator<Item = &Catalog> {
        self.order.iter().map(|&i| &self.catalogs[i as usize])
    }

    fn rebuild_order(&mut self) {
        let base = self.language.split('-').next().unwrap_or_default();
        let mut chain: Vec<&str> = Vec::with_capacity(3);
        for language in [self.language.as_str(), base, FALLBACK_LANGUAGE] {
            if !language.is_empty() && !chain.contains(&language) {
                chain.push(language);
            }
        }
        // Only catalogs in the chain keep their message index.
        for catalog in &mut self.catalogs {
            if chain.contains(&catalog.language()) {
                catalog.load();
            } else {
                catalog.unload();
            }
        }
        self.order.clear();
        for language in chain {
            let matching = self
                .catalogs
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, catalog)| catalog.language() == language)
                .map(|(i, _)| i as u16);
            self.order.extend(matching);
        }
        self.order.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = "[meta]\nlanguage = en\nname = English\nplural = one-other\n\n\
        [messages]\nhello = Hello, {name}!\nonly-en = English only\n\
        books.one = {count} book\nbooks.other = {count} books\n";
    const DE: &str = "[meta]\nlanguage = de\nname = Deutsch\nplural = one-other\n\n\
        [messages]\nhello = Hallo, {name}!\nbooks.one = {count} Buch\nbooks.other = {count} Bücher\n";
    const DE_AT: &str = "[meta]\nlanguage = de-AT\nname = Deutsch (Österreich)\n\n\
        [messages]\nhello = Servus, {name}!\n";
    const DE_OVERRIDE: &str = "[meta]\nlanguage = de\n[messages]\nhello = Moin, {name}!\n";

    fn translator(language: &str) -> Translator {
        let mut t = Translator::new();
        for source in [EN, DE, DE_AT] {
            t.add(Catalog::parse(source).unwrap());
        }
        t.set_language(language);
        t
    }

    #[test]
    fn falls_back_through_the_chain() {
        let t = translator("de-AT");
        assert_eq!(t.language(), "de-AT");
        assert_eq!(
            t.format("hello", &[("name", Arg::Str("Ada"))]),
            "Servus, Ada!"
        );
        assert_eq!(t.plural("books", 2, &[]), "2 Bücher");
        assert_eq!(t.tr("only-en"), "English only");
        assert_eq!(t.tr("missing.key"), "missing.key");
    }

    #[test]
    fn switches_language_at_runtime() {
        let mut t = translator("de");
        assert_eq!(t.plural("books", 1, &[]), "1 Buch");
        t.set_language("en");
        assert_eq!(t.plural("books", 1, &[]), "1 book");
        t.set_language("fr");
        assert_eq!(t.plural("books", 3, &[]), "3 books");
    }

    #[test]
    fn later_catalogs_override_earlier_ones() {
        let mut t = translator("de");
        t.add(Catalog::parse(DE_OVERRIDE).unwrap());
        assert_eq!(
            t.format("hello", &[("name", Arg::Str("Ada"))]),
            "Moin, Ada!"
        );
    }

    #[test]
    fn lists_languages_once() {
        let mut t = translator("de");
        t.add(Catalog::parse(DE_OVERRIDE).unwrap());
        let tags: Vec<&str> = t.languages().into_iter().map(|(tag, _)| tag).collect();
        assert_eq!(tags, ["de", "de-AT", "en"]);
    }

    #[test]
    fn missing_plural_returns_key() {
        assert_eq!(translator("de").plural("nothing", 3, &[]), "nothing");
    }
}

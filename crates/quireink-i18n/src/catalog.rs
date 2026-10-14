use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use quireink_config::ParseError;

use crate::plural::PluralRule;

/// The messages of one language.
///
/// All text is stored in a single buffer, with a sorted index of byte ranges,
/// to keep the number of allocations (and heap fragmentation on small
/// devices) minimal.
#[derive(Debug, Clone)]
pub struct Catalog {
    language: Range<u32>,
    name: Range<u32>,
    plural: PluralRule,
    text: String,
    /// `(key, message)` ranges into `text`, sorted by key.
    index: Vec<(Range<u32>, Range<u32>)>,
}

/// Why a catalog could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogError {
    Syntax(ParseError),
    /// `[meta] language` is missing.
    MissingLanguage,
    /// `[meta] plural` names an unknown rule.
    UnknownPluralRule {
        line: u32,
    },
    /// A key outside the `[meta]` and `[messages]` sections.
    UnexpectedSection {
        line: u32,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::Syntax(error) => error.fmt(f),
            CatalogError::MissingLanguage => f.write_str("missing `language` in [meta]"),
            CatalogError::UnknownPluralRule { line } => {
                write!(f, "line {line}: unknown plural rule")
            }
            CatalogError::UnexpectedSection { line } => {
                write!(f, "line {line}: only [meta] and [messages] are allowed")
            }
        }
    }
}

impl core::error::Error for CatalogError {}

impl Catalog {
    /// Parses a catalog file.
    pub fn parse(source: &str) -> Result<Catalog, CatalogError> {
        let mut text = String::new();
        let mut index = Vec::new();
        let mut language = None;
        let mut name = None;
        let mut plural = PluralRule::OneOther;

        for entry in quireink_config::parse(source) {
            let entry = entry.map_err(CatalogError::Syntax)?;
            let value = entry.value.as_str();
            match (entry.section, entry.key) {
                ("meta", "language") => language = Some(push(&mut text, &value)),
                ("meta", "name") => name = Some(push(&mut text, &value)),
                ("meta", "plural") => {
                    plural = PluralRule::from_id(&value)
                        .ok_or(CatalogError::UnknownPluralRule { line: entry.line })?;
                }
                ("meta", _) => {}
                ("messages", key) => {
                    let key = push(&mut text, key);
                    let message = push(&mut text, &value);
                    index.push((key, message));
                }
                _ => return Err(CatalogError::UnexpectedSection { line: entry.line }),
            }
        }

        let language = language.ok_or(CatalogError::MissingLanguage)?;
        // Stable sort keeps file order for duplicates; the last one wins below.
        index.sort_by(|a, b| text[range(&a.0)].cmp(&text[range(&b.0)]));
        index.dedup_by(|later, earlier| {
            let duplicate = text[range(&later.0)] == text[range(&earlier.0)];
            if duplicate {
                earlier.1 = later.1.clone();
            }
            duplicate
        });
        text.shrink_to_fit();
        index.shrink_to_fit();

        Ok(Catalog {
            name: name.unwrap_or_else(|| language.clone()),
            language,
            plural,
            text,
            index,
        })
    }

    /// Language tag, e.g. `de` or `pt-BR`.
    pub fn language(&self) -> &str {
        &self.text[range(&self.language)]
    }

    /// Human-readable language name in that language, e.g. `Deutsch`.
    pub fn name(&self) -> &str {
        &self.text[range(&self.name)]
    }

    pub fn plural_rule(&self) -> PluralRule {
        self.plural
    }

    /// The message for `key`, if this catalog has one.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.index
            .binary_search_by(|(k, _)| self.text[range(k)].cmp(key))
            .ok()
            .map(|i| &self.text[range(&self.index[i].1)])
    }

    /// All message keys, sorted.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.index.iter().map(|(key, _)| &self.text[range(key)])
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

fn push(text: &mut String, value: &str) -> Range<u32> {
    let start = text.len() as u32;
    text.push_str(value);
    start..text.len() as u32
}

fn range(r: &Range<u32>) -> Range<usize> {
    r.start as usize..r.end as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn parses_meta_and_messages() {
        let catalog = Catalog::parse(
            "[meta]\nlanguage = de\nname = Deutsch\n[messages]\nb = Zwei\na = Eins\n",
        )
        .unwrap();
        assert_eq!(catalog.language(), "de");
        assert_eq!(catalog.name(), "Deutsch");
        assert_eq!(catalog.get("a"), Some("Eins"));
        assert_eq!(catalog.get("b"), Some("Zwei"));
        assert_eq!(catalog.get("c"), None);
        assert_eq!(catalog.keys().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn last_duplicate_wins() {
        let catalog =
            Catalog::parse("[meta]\nlanguage = en\n[messages]\na = first\na = second\n").unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.get("a"), Some("second"));
    }

    #[test]
    fn rejects_invalid_catalogs() {
        assert_eq!(
            Catalog::parse("[messages]\na = b\n").unwrap_err(),
            CatalogError::MissingLanguage
        );
        assert_eq!(
            Catalog::parse("[meta]\nlanguage = en\nplural = klingon\n").unwrap_err(),
            CatalogError::UnknownPluralRule { line: 3 }
        );
        assert_eq!(
            Catalog::parse("[meta]\nlanguage = en\n[other]\na = b\n").unwrap_err(),
            CatalogError::UnexpectedSection { line: 4 }
        );
    }
}

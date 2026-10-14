use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use quireink_config::ParseError;

use crate::plural::PluralRule;

/// The messages of one language.
///
/// Messages are not copied: the catalog keeps its source (usually text
/// built into the program) and an index of byte ranges into it, 16 bytes
/// per message. Only values with escape sequences are stored decoded. A
/// catalog of an unused language can drop its index ([`Catalog::unload`])
/// and rebuild it when needed.
#[derive(Debug, Clone)]
pub struct Catalog {
    source: Cow<'static, str>,
    language: Span,
    name: Span,
    plural: PluralRule,
    /// Decoded values that contained escape sequences; the meta values
    /// first (`meta_len` bytes), which stay when the catalog is unloaded.
    escaped: String,
    meta_len: u32,
    /// Sorted by key; empty while unloaded.
    index: Vec<(Span, Span)>,
    loaded: bool,
}

/// A byte range in the source, or in `escaped` if the top bit is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    start: u32,
    len: u32,
}

const ESCAPED: u32 = 1 << 31;

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
    /// Parses a catalog file. Built-in catalogs (`&'static str`) are not
    /// copied.
    pub fn parse(source: impl Into<Cow<'static, str>>) -> Result<Catalog, CatalogError> {
        let mut catalog = Catalog {
            source: source.into(),
            language: Span { start: 0, len: 0 },
            name: Span { start: 0, len: 0 },
            plural: PluralRule::OneOther,
            escaped: String::new(),
            meta_len: 0,
            index: Vec::new(),
            loaded: false,
        };
        catalog.scan(true)?;
        Ok(catalog)
    }

    /// Reads the source. With `meta`, also reads and validates `[meta]`
    /// (otherwise only the messages are indexed again).
    fn scan(&mut self, meta: bool) -> Result<(), CatalogError> {
        let source: &str = &self.source;
        let base = source.as_ptr() as usize;
        let mut escaped = if meta {
            String::new()
        } else {
            let mut kept = core::mem::take(&mut self.escaped);
            kept.truncate(self.meta_len as usize);
            kept
        };
        let span = |escaped: &mut String, value: Cow<'_, str>| match value {
            Cow::Borrowed(text) => Span {
                start: (text.as_ptr() as usize - base) as u32,
                len: text.len() as u32,
            },
            Cow::Owned(text) => {
                let start = escaped.len() as u32 | ESCAPED;
                escaped.push_str(&text);
                Span {
                    start,
                    len: text.len() as u32,
                }
            }
        };
        let mut index = Vec::new();
        let (mut language, mut name, mut plural) = (None, None, PluralRule::OneOther);
        for entry in quireink_config::parse(source) {
            let entry = entry.map_err(CatalogError::Syntax)?;
            match (entry.section, entry.key) {
                ("meta", _) if !meta => {}
                ("meta", "language") => language = Some(span(&mut escaped, entry.value.as_str())),
                ("meta", "name") => name = Some(span(&mut escaped, entry.value.as_str())),
                ("meta", "plural") => {
                    plural = PluralRule::from_id(&entry.value.as_str())
                        .ok_or(CatalogError::UnknownPluralRule { line: entry.line })?;
                }
                ("meta", _) => {}
                ("messages", key) => {
                    let key = span(&mut escaped, Cow::Borrowed(key));
                    index.push((key, span(&mut escaped, entry.value.as_str())));
                }
                _ => return Err(CatalogError::UnexpectedSection { line: entry.line }),
            }
        }
        if meta {
            // Meta values are read before any message, so an escaped one
            // is at the start of `escaped`.
            self.language = language.ok_or(CatalogError::MissingLanguage)?;
            self.name = name.unwrap_or(self.language);
            self.plural = plural;
            self.meta_len = [self.language, self.name]
                .iter()
                .filter(|s| s.start & ESCAPED != 0)
                .map(|s| (s.start & !ESCAPED) + s.len)
                .max()
                .unwrap_or(0);
        }
        // Stable sort keeps file order for duplicates; the last one wins.
        index.sort_by(|a, b| text_of(source, &escaped, a.0).cmp(text_of(source, &escaped, b.0)));
        index.dedup_by(|later, earlier| {
            let duplicate =
                text_of(source, &escaped, later.0) == text_of(source, &escaped, earlier.0);
            if duplicate {
                earlier.1 = later.1;
            }
            duplicate
        });
        index.shrink_to_fit();
        escaped.shrink_to_fit();
        self.escaped = escaped;
        self.index = index;
        self.loaded = true;
        Ok(())
    }

    /// Drops the message index (keeps language, name and plural rule).
    pub fn unload(&mut self) {
        self.index = Vec::new();
        self.escaped.truncate(self.meta_len as usize);
        self.escaped.shrink_to_fit();
        self.loaded = false;
    }

    /// Rebuilds the message index after [`Catalog::unload`].
    pub fn load(&mut self) {
        if !self.loaded {
            // The source was validated when it was parsed.
            let _ = self.scan(false);
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    fn text(&self, span: Span) -> &str {
        text_of(&self.source, &self.escaped, span)
    }

    /// Language tag, e.g. `de` or `pt-BR`.
    pub fn language(&self) -> &str {
        self.text(self.language)
    }

    /// Human-readable language name in that language, e.g. `Deutsch`.
    pub fn name(&self) -> &str {
        self.text(self.name)
    }

    pub fn plural_rule(&self) -> PluralRule {
        self.plural
    }

    /// The message for `key`, if this catalog has one and is loaded.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.index
            .binary_search_by(|(k, _)| self.text(*k).cmp(key))
            .ok()
            .map(|i| self.text(self.index[i].1))
    }

    /// All message keys, sorted.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.index.iter().map(|(key, _)| self.text(*key))
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

fn text_of<'a>(source: &'a str, escaped: &'a str, span: Span) -> &'a str {
    let (text, start) = if span.start & ESCAPED != 0 {
        (escaped, span.start & !ESCAPED)
    } else {
        (source, span.start)
    };
    text.get(start as usize..(start + span.len) as usize)
        .unwrap_or_default()
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
    fn escaped_values_survive_unloading() {
        // Owned sources (e.g. loaded at runtime) work the same way.
        let mut catalog = Catalog::parse(String::from(
            r#"[meta]
language = "d\\e"
[messages]
b = plain
a = "two\nlines"
"#,
        ))
        .unwrap();
        assert_eq!(catalog.language(), r"d\e");
        assert_eq!(catalog.get("a"), Some("two\nlines"));
        catalog.unload();
        assert_eq!(catalog.get("a"), None);
        assert_eq!(catalog.language(), r"d\e");
        catalog.load();
        assert_eq!(catalog.get("a"), Some("two\nlines"));
        assert_eq!(catalog.get("b"), Some("plain"));
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

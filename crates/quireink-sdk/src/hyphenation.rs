//! Hyphenation patterns as contributions.

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

use quireink_hyphen::Patterns;

use crate::id::Id;
use crate::plugin::RegisterError;

/// Hyphenation patterns for a language, compiled with
/// [`hyphen::compile`](crate::hyphen::compile).
pub struct HyphenationSpec {
    pub id: Id,
    /// Language tag, e.g. `de` or `en-US`.
    pub language: Cow<'static, str>,
    pub patterns: &'static [u8],
}

impl fmt::Debug for HyphenationSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HyphenationSpec")
            .field("id", &self.id)
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

/// The registered hyphenation patterns.
#[derive(Debug, Default)]
pub struct Hyphenation {
    languages: Vec<(Id, Cow<'static, str>, Patterns<'static>)>,
}

impl Hyphenation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, spec: HyphenationSpec) -> Result<(), RegisterError> {
        if self.languages.iter().any(|(id, ..)| *id == spec.id) {
            return Err(RegisterError::Duplicate { id: spec.id });
        }
        let patterns = Patterns::new(spec.patterns).map_err(|e| RegisterError::Invalid {
            message: Cow::Owned(alloc::format!("{}: {e}", spec.id)),
        })?;
        self.languages.push((spec.id, spec.language, patterns));
        Ok(())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.languages.iter().any(|(i, ..)| i.as_str() == id)
    }

    /// Releases unused capacity once registration is complete.
    pub fn shrink_to_fit(&mut self) {
        self.languages.shrink_to_fit();
    }

    /// Patterns for `language` (e.g. `de-AT`), falling back to its base
    /// language (`de`); later registrations win.
    pub fn get(&self, language: &str) -> Option<&Patterns<'static>> {
        let base = language.split(['-', '_']).next().unwrap_or(language);
        let find = |tag: &str| {
            self.languages
                .iter()
                .rev()
                .find(|(_, l, _)| l.eq_ignore_ascii_case(tag))
                .map(|(.., p)| p)
        };
        find(language).or_else(|| {
            // `en` also serves `en-US`, and `en-US` serves `en`.
            find(base).or_else(|| {
                self.languages
                    .iter()
                    .rev()
                    .find(|(_, l, _)| {
                        l.split(['-', '_'])
                            .next()
                            .is_some_and(|b| b.eq_ignore_ascii_case(base))
                    })
                    .map(|(.., p)| p)
            })
        })
    }
}

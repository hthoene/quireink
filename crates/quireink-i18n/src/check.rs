//! Consistency checks between a translation and the reference (English)
//! catalog. Used by tests in every plugin, so translators get precise
//! feedback in CI.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::catalog::Catalog;
use crate::plural::PluralCategory;

/// A problem found in a translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// The reference has a message the translation lacks.
    Missing(String),
    /// The translation has a message the reference does not know.
    Unknown(String),
    /// The placeholders of a message differ from the reference.
    Placeholders(String),
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::Missing(key) => write!(f, "missing message `{key}`"),
            Issue::Unknown(key) => write!(f, "unknown message `{key}`"),
            Issue::Placeholders(key) => write!(f, "placeholders of `{key}` differ"),
        }
    }
}

/// Compares `translation` against `reference`.
///
/// Plural messages (`key.one`, `key.few`, …) are compared by their base key,
/// because languages use different sets of plural categories.
pub fn compare(reference: &Catalog, translation: &Catalog) -> Vec<Issue> {
    let groups = |catalog: &Catalog| -> BTreeSet<String> {
        catalog
            .keys()
            .map(|key| String::from(base_key(key)))
            .collect()
    };
    let expected = groups(reference);
    let actual = groups(translation);

    let mut issues: Vec<Issue> = expected
        .difference(&actual)
        .map(|key| Issue::Missing(key.clone()))
        .chain(
            actual
                .difference(&expected)
                .map(|key| Issue::Unknown(key.clone())),
        )
        .collect();

    for key in translation.keys() {
        let reference_message = reference
            .get(key)
            .or_else(|| reference.get(&alloc::format!("{}.other", base_key(key))));
        if let (Some(reference_message), Some(message)) = (reference_message, translation.get(key))
            && placeholders(reference_message) != placeholders(message)
        {
            issues.push(Issue::Placeholders(String::from(key)));
        }
    }
    issues
}

fn base_key(key: &str) -> &str {
    const CATEGORIES: [PluralCategory; 6] = [
        PluralCategory::Zero,
        PluralCategory::One,
        PluralCategory::Two,
        PluralCategory::Few,
        PluralCategory::Many,
        PluralCategory::Other,
    ];
    match key.rsplit_once('.') {
        Some((base, suffix)) if CATEGORIES.iter().any(|c| c.id() == suffix) => base,
        _ => key,
    }
}

fn placeholders(message: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        let tail = &rest[open + 1..];
        if let Some(stripped) = tail.strip_prefix('{') {
            rest = stripped;
            continue;
        }
        match tail.find('}') {
            Some(close) => {
                names.insert(&tail[..close]);
                rest = &tail[close + 1..];
            }
            None => break,
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = "[meta]\nlanguage = en\n[messages]\n\
        a = A\nb = Hello {name}\nitems.one = {count} item\nitems.other = {count} items\n";

    #[test]
    fn detects_missing_unknown_and_placeholder_issues() {
        let reference = Catalog::parse(EN).unwrap();
        let translation = Catalog::parse(
            "[meta]\nlanguage = ru\nplural = east-slavic\n[messages]\n\
             b = Привет {nmae}\nitems.one = {count} предмет\nitems.few = {count} предмета\n\
             items.many = {count} предметов\nextra = X\n",
        )
        .unwrap();
        let issues = compare(&reference, &translation);
        assert!(issues.contains(&Issue::Missing("a".into())));
        assert!(issues.contains(&Issue::Unknown("extra".into())));
        assert!(issues.contains(&Issue::Placeholders("b".into())));
        assert_eq!(issues.len(), 3, "{issues:?}");
    }
}

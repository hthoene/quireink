//! Hyphenation patterns for German (reformed spelling) and English
//! (American spelling).
//!
//! The patterns are compiled at build time and read in place, so they cost
//! flash but almost no RAM. Other languages are separate plugins that add
//! their own patterns the same way; a later registration for a language
//! replaces an earlier one.

#![no_std]

use alloc::borrow::Cow;

extern crate alloc;

use quireink_sdk::{
    HyphenationSpec, Id, Manifest, Plugin, RegisterError, Registrar, SDK_VERSION, Version,
};

pub const PLUGIN: Id = Id::new_static("hyphenation");
pub const GERMAN: Id = Id::new_static("hyphenation.de");
pub const ENGLISH: Id = Id::new_static("hyphenation.en");

const DE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/de.qihp"));
const EN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/en.qihp"));

/// The hyphenation plugin.
#[derive(Debug, Default)]
pub struct Hyphenation;

impl Plugin for Hyphenation {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Hyphenation (German, English)"),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        for (id, language, patterns) in [(GERMAN, "de", DE), (ENGLISH, "en", EN)] {
            r.add_hyphenation(HyphenationSpec {
                id,
                language: Cow::Borrowed(language),
                patterns,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;
    use quireink_sdk::hyphen::Patterns;

    fn hyphenated(table: &[u8], word: &str) -> String {
        let patterns = Patterns::new(table).unwrap();
        let mut breaks = Vec::new();
        patterns.hyphenate(word, &mut |at| breaks.push(at));
        let mut out = String::new();
        for (i, c) in word.char_indices() {
            if breaks.contains(&i) {
                out.push('-');
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn hyphenates_german() {
        assert_eq!(hyphenated(DE, "Verwandlung"), "Ver-wand-lung");
        assert_eq!(hyphenated(DE, "Musterkollektion"), "Mus-ter-kol-lek-ti-on");
        assert_eq!(hyphenated(DE, "Ungeziefer"), "Un-ge-zie-fer");
        assert_eq!(hyphenated(DE, "Rücken"), "Rü-cken");
    }

    #[test]
    fn hyphenates_english() {
        assert_eq!(hyphenated(EN, "hyphenation"), "hy-phen-ation");
        assert_eq!(hyphenated(EN, "Wonderland"), "Won-der-land");
        // From the exception list.
        assert_eq!(hyphenated(EN, "table"), "ta-ble");
    }
}

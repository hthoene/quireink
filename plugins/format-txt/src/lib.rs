//! Plain text (`.txt`) support for QuireInk.
//!
//! - Encodings: UTF-8 (with or without BOM); otherwise Windows-1252.
//! - Paragraphs: separated by blank lines when the text uses them (typical
//!   for hard-wrapped e-texts), otherwise one paragraph per line.
//! - Project Gutenberg files: the licence header and footer are skipped and
//!   title, author and language are read from the header.
//!
//! The file is never loaded completely; each request reads a few kilobytes
//! around the position, so arbitrarily large files work on small devices.

#![no_std]

extern crate alloc;

mod document;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec;

use quireink_sdk::document::Document;
use quireink_sdk::{
    FormatSpec, Id, Manifest, Plugin, RegisterError, Registrar, SDK_VERSION, Version,
};

pub use document::TxtDocument;

pub const PLUGIN: Id = Id::new_static("format-txt");
pub const FORMAT: Id = Id::new_static("format-txt.txt");

/// The plain text format plugin.
#[derive(Debug, Default)]
pub struct TxtFormat;

impl Plugin for TxtFormat {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Plain text"),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_format(FormatSpec {
            id: FORMAT,
            title: Cow::Borrowed("format-txt.title"),
            extensions: vec![Cow::Borrowed("txt"), Cow::Borrowed("text")],
            open: Box::new(|storage, path| {
                TxtDocument::open(storage, path).map(|d| Box::new(d) as Box<dyn Document>)
            }),
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

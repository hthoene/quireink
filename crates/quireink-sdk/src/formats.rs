use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use quireink_document::{Document, DocumentError};
use quireink_platform::Storage;

use crate::id::Id;
use crate::plugin::RegisterError;

/// Opens a file of a format as a [`Document`].
pub type FormatOpener = Box<dyn Fn(&dyn Storage, &str) -> Result<Box<dyn Document>, DocumentError>>;

/// A file format contribution (TXT, EPUB, FB2, …).
pub struct FormatSpec {
    /// `<plugin>.<name>`, e.g. `format-txt.txt`.
    pub id: Id,
    /// Translation key of the format's display name.
    pub title: Cow<'static, str>,
    /// Lowercase file extensions without the dot, e.g. `["txt", "text"]`.
    pub extensions: Vec<Cow<'static, str>>,
    pub open: FormatOpener,
}

impl fmt::Debug for FormatSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormatSpec")
            .field("id", &self.id)
            .field("extensions", &self.extensions)
            .finish_non_exhaustive()
    }
}

/// All registered file formats.
#[derive(Debug, Default)]
pub struct Formats {
    specs: Vec<FormatSpec>,
}

impl Formats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a format. Later registrations win for shared extensions,
    /// so a plugin can replace a built-in format.
    pub fn add(&mut self, spec: FormatSpec) -> Result<(), RegisterError> {
        if self.specs.iter().any(|s| s.id == spec.id) {
            return Err(RegisterError::Duplicate { id: spec.id });
        }
        self.specs.push(spec);
        Ok(())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.specs.iter().any(|s| s.id.as_str() == id)
    }

    /// The format handling `path`, by file extension.
    pub fn for_path(&self, path: &str) -> Option<&FormatSpec> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let (_, extension) = name.rsplit_once('.')?;
        self.specs.iter().rev().find(|spec| {
            spec.extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(extension))
        })
    }

    /// Whether any format can open `path`.
    pub fn supports(&self, path: &str) -> bool {
        self.for_path(path).is_some()
    }

    /// Opens `path` with the matching format.
    pub fn open(
        &self,
        storage: &dyn Storage,
        path: &str,
    ) -> Result<Box<dyn Document>, DocumentError> {
        let spec = self
            .for_path(path)
            .ok_or(DocumentError::UnsupportedFormat)?;
        (spec.open)(storage, path)
    }

    pub fn iter(&self) -> impl Iterator<Item = &FormatSpec> {
        self.specs.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn spec(id: &'static str, ext: &'static str) -> FormatSpec {
        FormatSpec {
            id: Id::new_static(id),
            title: Cow::Borrowed(""),
            extensions: vec![Cow::Borrowed(ext)],
            open: Box::new(|_, _| Err(DocumentError::UnsupportedFormat)),
        }
    }

    #[test]
    fn finds_formats_by_extension_and_later_wins() {
        let mut formats = Formats::new();
        formats.add(spec("a.txt", "txt")).unwrap();
        formats.add(spec("b.txt", "txt")).unwrap();
        assert_eq!(
            formats.for_path("books/Alice.TXT").map(|s| s.id.as_str()),
            Some("b.txt")
        );
        assert!(!formats.supports("books/alice"));
        assert!(!formats.supports("books.d/alice"));
        assert!(formats.add(spec("a.txt", "md")).is_err());
    }
}

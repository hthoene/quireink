use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use quireink_document::{Document, DocumentError};
use quireink_platform::Storage;

use crate::id::Id;
use crate::plugin::RegisterError;

/// Opens a file of a format as a [`Document`].
pub type FormatOpener =
    Box<dyn Fn(&dyn Storage, &str, &OpenOptions) -> Result<Box<dyn Document>, DocumentError>>;

/// How a document is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenOptions {
    /// Apply the book's own styles (e.g. the CSS of an EPUB). Without
    /// them the text keeps only its markup: emphasis, headings, lists.
    pub publisher_styles: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            publisher_styles: true,
        }
    }
}

impl OpenOptions {
    pub fn publisher_styles(mut self, on: bool) -> Self {
        self.publisher_styles = on;
        self
    }
}

/// A file format contribution (TXT, EPUB, FB2, …).
pub struct FormatSpec {
    /// `<plugin>.<name>`, e.g. `format-txt.txt`.
    pub id: Id,
    /// Translation key of the format's display name.
    pub title: Cow<'static, str>,
    /// Lowercase file extensions without the dot, e.g. `["txt", "text"]`;
    /// also double ones such as `fb2.zip`.
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

    /// Releases unused capacity once registration is complete.
    pub fn shrink_to_fit(&mut self) {
        self.specs.shrink_to_fit();
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

    /// The format handling `path`, by file extension (the longest that
    /// matches, so `book.fb2.zip` goes to `fb2.zip`).
    pub fn for_path(&self, path: &str) -> Option<&FormatSpec> {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let matches = |e: &str| {
            name.len() > e.len()
                && name.ends_with(&e.to_ascii_lowercase())
                && name.as_bytes()[name.len() - e.len() - 1] == b'.'
        };
        let mut best: Option<(&FormatSpec, usize)> = None;
        for spec in self.specs.iter().rev() {
            for e in spec.extensions.iter().filter(|e| matches(e)) {
                if best.is_none_or(|(_, len)| e.len() > len) {
                    best = Some((spec, e.len()));
                }
            }
        }
        best.map(|(spec, _)| spec)
    }

    /// Whether any format can open `path`.
    pub fn supports(&self, path: &str) -> bool {
        self.for_path(path).is_some()
    }

    /// Opens `path` with the matching format, as the book intends.
    pub fn open(
        &self,
        storage: &dyn Storage,
        path: &str,
    ) -> Result<Box<dyn Document>, DocumentError> {
        self.open_with(storage, path, &OpenOptions::default())
    }

    /// Opens `path` with the matching format and `options`.
    pub fn open_with(
        &self,
        storage: &dyn Storage,
        path: &str,
        options: &OpenOptions,
    ) -> Result<Box<dyn Document>, DocumentError> {
        let spec = self
            .for_path(path)
            .ok_or(DocumentError::UnsupportedFormat)?;
        (spec.open)(storage, path, options)
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
            open: Box::new(|_, _, _| Err(DocumentError::UnsupportedFormat)),
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

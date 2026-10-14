//! The format-independent document model of QuireInk.
//!
//! Format plugins (TXT, EPUB, FB2, …) turn files into [`Document`]s; layout
//! engines turn documents into pages. Neither side knows the other.
//!
//! - A document consists of **sections** (one for TXT, one per spine item
//!   for EPUB).
//! - Each section is a sequence of **blocks** (paragraphs, headings, …)
//!   with normalized text.
//! - A [`Locator`] identifies a position by section and *source offset* — a
//!   position in the original file data, which stays valid regardless of
//!   fonts, screen size or text normalization (see ADR 0007).
//!
//! Documents are read block by block, so even very large books need only a
//! few kilobytes of RAM.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use quireink_platform::StorageError;

pub use quireink_platform::{DirEntry, NoStorage, Storage};

/// A reading position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Locator {
    pub section: u32,
    /// Offset in the section's source data (e.g. bytes of a TXT file).
    pub offset: u32,
}

impl Locator {
    pub const START: Locator = Locator {
        section: 0,
        offset: 0,
    };

    pub const fn new(section: u32, offset: u32) -> Self {
        Self { section, offset }
    }
}

/// Bibliographic information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    /// BCP 47 language tag, e.g. `de` or `en-GB`.
    pub language: Option<String>,
}

/// The role of a block; layout engines style blocks by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlockKind {
    Paragraph,
    /// Heading level 1 (most important) to 6.
    Heading(u8),
    /// Text whose line breaks and spacing must be kept.
    Preformatted,
}

/// A block of text, e.g. a paragraph.
///
/// `text` is normalized (decoded to UTF-8, whitespace collapsed); `map`
/// converts between byte indices into `text` and source offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// The block continues the previous one (a very long paragraph split
    /// into pieces): no first-line indent, no paragraph spacing.
    pub continues: bool,
    /// Source offset of the block's first byte.
    pub start: u32,
    /// Source offset just after the block.
    pub end: u32,
    pub text: String,
    pub map: OffsetMap,
}

impl Block {
    /// The source offset of byte `index` of `text`.
    pub fn source_offset(&self, index: usize) -> u32 {
        if index >= self.text.len() {
            return self.end;
        }
        self.map.source_offset(self.start, index)
    }

    /// The byte index in `text` for a source offset inside the block
    /// (rounded down to a character boundary).
    pub fn text_index(&self, offset: u32) -> usize {
        if offset <= self.start {
            return 0;
        }
        if offset >= self.end {
            return self.text.len();
        }
        let mut index = self.map.text_index(self.start, offset).min(self.text.len());
        while !self.text.is_char_boundary(index) {
            index -= 1;
        }
        index
    }
}

/// Maps text indices of a block to source offsets.
///
/// Normalization (dropping `\r`, decoding legacy encodings, collapsing
/// spaces) shifts positions. Instead of storing an offset per byte, the map
/// stores checkpoints where the shift changes; between checkpoints both
/// advance in lockstep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OffsetMap {
    /// `(text index, source offset)` pairs, sorted by text index.
    checkpoints: Vec<(u32, u32)>,
}

impl OffsetMap {
    /// Text and source advance in lockstep from the block start.
    pub const IDENTITY: OffsetMap = OffsetMap {
        checkpoints: Vec::new(),
    };

    /// Records that text index `text` corresponds to source offset `source`.
    /// Checkpoints must be added in increasing order.
    pub fn checkpoint(&mut self, text: usize, source: u32) {
        let text = text as u32;
        if let Some(last) = self.checkpoints.last_mut()
            && last.0 == text
        {
            last.1 = source;
            return;
        }
        self.checkpoints.push((text, source));
    }

    fn source_offset(&self, start: u32, index: usize) -> u32 {
        let index = index as u32;
        let i = self.checkpoints.partition_point(|&(text, _)| text <= index);
        match i.checked_sub(1).map(|i| self.checkpoints[i]) {
            Some((text, source)) => source + (index - text),
            None => start + index,
        }
    }

    fn text_index(&self, start: u32, offset: u32) -> usize {
        let i = self
            .checkpoints
            .partition_point(|&(_, source)| source <= offset);
        match i.checked_sub(1).map(|i| self.checkpoints[i]) {
            Some((text, source)) => {
                // Inside a gap (e.g. a dropped `\r`) map to the checkpoint.
                let next_text = self.checkpoints.get(i).map_or(u32::MAX, |c| c.0);
                (text + (offset - source)).min(next_text) as usize
            }
            None => offset.saturating_sub(start) as usize,
        }
    }
}

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentError {
    Storage(StorageError),
    /// The file is not valid for this format.
    Malformed(Cow<'static, str>),
    /// No format plugin can open the file.
    UnsupportedFormat,
    /// The device has too little free memory for this document.
    OutOfMemory,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::Storage(error) => write!(f, "storage: {error}"),
            DocumentError::Malformed(message) => write!(f, "malformed document: {message}"),
            DocumentError::UnsupportedFormat => f.write_str("unsupported format"),
            DocumentError::OutOfMemory => f.write_str("not enough memory"),
        }
    }
}

impl core::error::Error for DocumentError {}

impl From<StorageError> for DocumentError {
    fn from(error: StorageError) -> Self {
        DocumentError::Storage(error)
    }
}

/// An entry of a document's table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub title: String,
    /// Nesting depth, 0 for top-level entries.
    pub level: u8,
    pub at: Locator,
}

/// An open document.
pub trait Document {
    fn metadata(&self) -> &Metadata;

    /// Number of sections (at least 1).
    fn sections(&self) -> u32;

    /// Length of a section in source units, for progress calculation.
    fn section_len(&self, section: u32) -> u32;

    /// The block containing `at`, or the first block after it. `None` when
    /// `at` is at or past the end of its section.
    ///
    /// Block boundaries must not depend on `at`: every position inside a
    /// block yields the same block.
    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError>;

    /// The table of contents; empty if the format or file has none.
    fn toc(&self, storage: &dyn Storage) -> Vec<TocEntry> {
        let _ = storage;
        Vec::new()
    }

    /// Reading progress of `at` from 0.0 to 1.0.
    fn progress(&self, at: Locator) -> f32 {
        let total: u64 = (0..self.sections())
            .map(|s| u64::from(self.section_len(s)))
            .sum();
        if total == 0 {
            return 0.0;
        }
        let before: u64 = (0..at.section.min(self.sections()))
            .map(|s| u64::from(self.section_len(s)))
            .sum();
        ((before + u64::from(at.offset)) as f32 / total as f32).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, start: u32, end: u32, map: OffsetMap) -> Block {
        Block {
            kind: BlockKind::Paragraph,
            continues: false,
            start,
            end,
            text: String::from(text),
            map,
        }
    }

    #[test]
    fn identity_mapping() {
        let b = block("hello world", 100, 111, OffsetMap::IDENTITY);
        assert_eq!(b.source_offset(6), 106);
        assert_eq!(b.text_index(106), 6);
        assert_eq!(b.source_offset(99), 111);
        assert_eq!(b.text_index(50), 0);
        assert_eq!(b.text_index(500), 11);
    }

    #[test]
    fn checkpoints_account_for_dropped_bytes() {
        // Source "ab\r\ncd" at 0 → text "ab cd": "\r\n" became one space.
        let mut map = OffsetMap::default();
        map.checkpoint(3, 4);
        let b = block("ab cd", 0, 6, map);
        assert_eq!(b.source_offset(1), 1);
        assert_eq!(b.source_offset(2), 2);
        assert_eq!(b.source_offset(3), 4);
        assert_eq!(b.source_offset(4), 5);
        assert_eq!(b.text_index(4), 3);
        assert_eq!(b.text_index(5), 4);
        assert_eq!(b.text_index(3), 3);
    }

    #[test]
    fn text_index_respects_char_boundaries() {
        // "ä" is two bytes in UTF-8 but one byte in Windows-1252.
        let mut map = OffsetMap::default();
        map.checkpoint(3, 2); // after "xä"
        let b = block("xäy", 0, 3, map);
        assert_eq!(b.text_index(2), 3);
        assert_eq!(b.source_offset(3), 2);
        assert!(b.text.is_char_boundary(b.text_index(1)));
    }
}

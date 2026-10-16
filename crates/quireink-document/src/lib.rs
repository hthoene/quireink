//! The format-independent document model of QuireInk.
//!
//! Format plugins (TXT, EPUB, FB2, …) turn files into [`Document`]s; layout
//! engines turn documents into pages. Neither side knows the other.
//!
//! - A document consists of **sections** (one for TXT, one per spine item
//!   for EPUB).
//! - Each section is a sequence of **blocks** (paragraphs, headings, …)
//!   with normalized text.
//! - A [`Locator`] identifies a position by section and *source offset*, a
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
    /// The series the book belongs to, if it says so.
    pub series: Option<String>,
    /// Its number in the series, e.g. `2` or `2.5`, as written.
    pub series_index: Option<String>,
    /// Its subjects or tags, e.g. an EPUB's `dc:subject` (where Calibre
    /// puts its tags).
    pub subjects: Vec<String>,
    /// What the book is about, as plain text (a blurb), if it says.
    pub description: Option<String>,
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
    /// An image ([`Block::image`]); `text` is its alternative text.
    Image,
}

/// A decoded image in storage, in the `quireink-image` gray format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// The character style of a run of text: bold, italic, small capitals
/// and the font family embedded in the document, if any (see
/// [`Document::fonts`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextStyle(u8);

/// Most embedded font families a document can use.
pub const MAX_FAMILIES: u8 = 31;

/// The bits of bold, italic and small capitals; the family is in the
/// others.
const FLAGS: u8 = 0b1000_0011;

impl TextStyle {
    pub const REGULAR: TextStyle = TextStyle(0);
    pub const BOLD: TextStyle = TextStyle(1);
    pub const ITALIC: TextStyle = TextStyle(2);
    pub const BOLD_ITALIC: TextStyle = TextStyle(3);
    /// Small capitals: lower case letters set as smaller capitals.
    pub const SMALL_CAPS: TextStyle = TextStyle(0b1000_0000);

    pub const fn bold(self) -> bool {
        self.0 & 1 != 0
    }

    pub const fn italic(self) -> bool {
        self.0 & 2 != 0
    }

    pub const fn small_caps(self) -> bool {
        self.0 & Self::SMALL_CAPS.0 != 0
    }

    /// The embedded font family (1 to [`MAX_FAMILIES`]); 0 for the
    /// reader's font.
    pub const fn family(self) -> u8 {
        (self.0 >> 2) & MAX_FAMILIES
    }

    /// This style in embedded family `family` (0: the reader's font).
    pub const fn with_family(self, family: u8) -> TextStyle {
        let family = if family > MAX_FAMILIES { 0 } else { family };
        TextStyle((self.0 & FLAGS) | (family << 2))
    }

    /// Both styles combined, e.g. bold text in an italic paragraph; the
    /// family is `other`'s if it has one.
    pub const fn union(self, other: TextStyle) -> TextStyle {
        let family = if other.family() != 0 {
            other.family()
        } else {
            self.family()
        };
        TextStyle((self.0 | other.0) & FLAGS).with_family(family)
    }

    /// For storing styles compactly.
    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn from_bits(bits: u8) -> TextStyle {
        TextStyle(bits)
    }
}

/// A run of styled text: byte range `start..end` of a block's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub start: u32,
    pub end: u32,
    pub style: TextStyle,
}

/// Horizontal alignment of a block's lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Align {
    /// As the reader prefers (justified or flush left).
    #[default]
    Auto,
    Start,
    Center,
    End,
    Justify,
}

impl Align {
    /// For storing alignments compactly.
    pub const fn bits(self) -> u8 {
        match self {
            Align::Auto => 0,
            Align::Start => 1,
            Align::Center => 2,
            Align::End => 3,
            Align::Justify => 4,
        }
    }

    pub const fn from_bits(bits: u8) -> Align {
        match bits {
            1 => Align::Start,
            2 => Align::Center,
            3 => Align::End,
            4 => Align::Justify,
            _ => Align::Auto,
        }
    }
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
    /// Styled runs of `text`, sorted and not overlapping; text outside any
    /// run is regular.
    pub runs: Vec<Run>,
    pub align: Align,
    /// The book's indent of the first line, in hundredths of an em;
    /// `None` leaves it to the reader.
    pub indent: Option<i16>,
    /// The book's space above the block, in hundredths of an em; `None`
    /// leaves it to the reader.
    pub space: Option<u16>,
    /// The book's text size, in percent of body text; `None` leaves it to
    /// the reader (body text, or by heading level).
    pub size: Option<u16>,
    /// The image of an [`BlockKind::Image`] block.
    pub image: Option<ImageRef>,
}

impl Block {
    /// The style of the character at byte `index` of `text`.
    pub fn style_at(&self, index: usize) -> TextStyle {
        let index = index as u32;
        let at = self.runs.partition_point(|run| run.end <= index);
        match self.runs.get(at) {
            Some(run) if run.start <= index => run.style,
            _ => TextStyle::REGULAR,
        }
    }

    /// Replaces bytes `range` of `text` (on character boundaries) with
    /// `with`, e.g. in a text filter: styles move along, and positions in
    /// the source stay right around it (the new text maps to where the old
    /// one was).
    pub fn replace(&mut self, range: core::ops::Range<usize>, with: &str) {
        let (from, to) = (range.start, range.end.min(self.text.len()));
        if from > to || !self.text.is_char_boundary(from) || !self.text.is_char_boundary(to) {
            return;
        }
        let shift = |i: u32| -> u32 { (i as usize + with.len() - (to - from)) as u32 };
        let (source_from, source_to) = (self.source_offset(from), self.source_offset(to));
        let mut checkpoints: Vec<(u32, u32)> = self
            .map
            .checkpoints
            .iter()
            .copied()
            .filter(|&(t, _)| (t as usize) < from)
            .collect();
        checkpoints.push((from as u32, source_from));
        checkpoints.push(((from + with.len()) as u32, source_to));
        checkpoints.extend(
            self.map
                .checkpoints
                .iter()
                .filter(|&&(t, _)| t as usize > to)
                .map(|&(t, s)| (shift(t), s)),
        );
        // One checkpoint per text index, the last one wins.
        let mut map = OffsetMap::default();
        for (t, s) in checkpoints {
            map.checkpoint(t as usize, s);
        }
        self.map = map;
        let end = (from + with.len()) as u32;
        for run in &mut self.runs {
            let (start, stop) = (run.start as usize, run.end as usize);
            if stop <= from {
                continue;
            }
            run.start = if start >= to {
                shift(run.start)
            } else {
                run.start.min(from as u32)
            };
            run.end = if stop >= to { shift(run.end) } else { end };
        }
        self.runs.retain(|run| run.start < run.end);
        self.text.replace_range(from..to, with);
    }

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

/// A font embedded in a document, e.g. with CSS `@font-face` in an EPUB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedFont {
    /// The family, as in [`TextStyle::family`].
    pub family: u8,
    pub bold: bool,
    pub italic: bool,
}

/// An entry of a document's table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub title: String,
    /// Nesting depth, 0 for top-level entries.
    pub level: u8,
    pub at: Locator,
}

/// A link in a section's text: source offsets `start..end`, and where it
/// goes (see [`Document::link_target`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub start: u32,
    pub end: u32,
    pub href: String,
}

/// Whether `href` leads out of the document, e.g. to a web page: it has a
/// scheme such as `https:` or `mailto:`.
pub fn is_external(href: &str) -> bool {
    href.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// An open document.
pub trait Document {
    fn metadata(&self) -> &Metadata;

    /// Number of sections (at least 1).
    fn sections(&self) -> u32;

    /// The page of the printed edition at `at`, as the book names it
    /// (an EPUB's `page-list`), e.g. `123` or `xii`; `None` if it names
    /// none there.
    fn page_label(&self, storage: &dyn Storage, at: Locator) -> Option<String> {
        let _ = (storage, at);
        None
    }

    /// Each section is the picture of a whole page (a comic, pages made
    /// for one screen): readers show them edge to edge, without margins
    /// or status lines.
    fn whole_pages(&self) -> bool {
        false
    }

    /// Length of a section in source units, for progress calculation.
    fn section_len(&self, section: u32) -> u32;

    /// The block containing `at`, or the first block after it. `None` when
    /// `at` is at or past the end of its section.
    ///
    /// Block boundaries must not depend on `at`: every position inside a
    /// block yields the same block.
    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError>;

    /// Number of table of contents entries; 0 if the format or file has
    /// none. Tables of contents can be long, so they are read entry by
    /// entry instead of all at once.
    fn toc_len(&self, storage: &dyn Storage) -> u32 {
        let _ = storage;
        0
    }

    /// Table of contents entry `index`, in reading order. Its locator may
    /// point at the start of the entry's section when the exact position
    /// is costly to find (e.g. needs converting a chapter); use
    /// [`Document::toc_target`] before jumping there.
    fn toc_entry(&self, storage: &dyn Storage, index: u32) -> Option<TocEntry> {
        let _ = (storage, index);
        None
    }

    /// The exact position of table of contents entry `index`.
    fn toc_target(&self, storage: &dyn Storage, index: u32) -> Option<Locator> {
        self.toc_entry(storage, index).map(|entry| entry.at)
    }

    /// The cover as a small gray image in storage, decoded on first use;
    /// `None` if the book has none (or the format no covers).
    fn cover(&self, storage: &dyn Storage) -> Result<Option<ImageRef>, DocumentError> {
        let _ = storage;
        Ok(None)
    }

    /// Position `at` as other readers write it, to sync reading positions
    /// with them: for EPUB, KOReader's XPointer to the paragraph (e.g.
    /// `/body/DocFragment[3]/body/div[2]/p[5]`). `None` where the format
    /// has no such positions.
    fn xpointer(&self, storage: &dyn Storage, at: Locator) -> Option<String> {
        let _ = (storage, at);
        None
    }

    /// The position of an XPointer (see [`Document::xpointer`]), to the
    /// precision of a paragraph; `None` if it does not fit the document.
    fn locate_xpointer(&self, storage: &dyn Storage, xpointer: &str) -> Option<Locator> {
        let _ = (storage, xpointer);
        None
    }

    /// The fonts the document embeds; text in their families carries the
    /// family in its [`TextStyle`].
    fn fonts(&self, storage: &dyn Storage) -> Vec<EmbeddedFont> {
        let _ = storage;
        Vec::new()
    }

    /// The data (an OpenType/TrueType file) of embedded font `index` of
    /// [`Document::fonts`].
    fn font_data(&self, storage: &dyn Storage, index: usize) -> Result<Vec<u8>, DocumentError> {
        let _ = (storage, index);
        Err(DocumentError::Malformed(Cow::Borrowed("no embedded fonts")))
    }

    /// The links of `section` that overlap source offsets `from..to`, in
    /// order.
    fn links(&self, storage: &dyn Storage, section: u32, from: u32, to: u32) -> Vec<Link> {
        let _ = (storage, section, from, to);
        Vec::new()
    }

    /// Where link `href` (of [`Document::links`]) leads in the document;
    /// `None` for links out of it ([`is_external`]) or to nothing.
    fn link_target(&self, storage: &dyn Storage, href: &str) -> Option<Locator> {
        let _ = (storage, href);
        None
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

    #[test]
    fn replacing_keeps_styles_and_positions() {
        let mut block = Block {
            kind: BlockKind::Paragraph,
            continues: false,
            start: 100,
            end: 120,
            text: String::from("Mr. Smith is here."),
            map: OffsetMap::IDENTITY,
            runs: alloc::vec![Run {
                start: 4,
                end: 9,
                style: TextStyle::BOLD,
            }],
            align: Align::default(),
            indent: None,
            space: None,
            size: None,
            image: None,
        };
        block.replace(0..3, "Mister");
        assert_eq!(block.text, "Mister Smith is here.");
        // "Smith" is still bold and still where it was in the book.
        assert_eq!(block.runs[0].start, 7);
        assert_eq!(block.runs[0].end, 12);
        assert_eq!(block.source_offset(7), 104);
        assert_eq!(block.source_offset(0), 100);
        assert_eq!(block.text_index(104), 7);
        assert_eq!(block.source_offset(block.text.len() - 1), 117);
    }
    use super::*;

    #[test]
    fn small_caps_keep_bold_italic_and_family() {
        let style = TextStyle::SMALL_CAPS
            .union(TextStyle::BOLD)
            .with_family(MAX_FAMILIES);
        assert!(style.small_caps() && style.bold() && !style.italic());
        assert_eq!(style.family(), MAX_FAMILIES);
        assert_eq!(TextStyle::from_bits(style.bits()), style);
        // Too many families: the reader's font, the flags kept.
        let other = TextStyle::SMALL_CAPS.with_family(MAX_FAMILIES + 1);
        assert!(other.small_caps());
        assert_eq!(other.family(), 0);
        assert!(!TextStyle::ITALIC.with_family(5).small_caps());
    }

    fn block(text: &str, start: u32, end: u32, map: OffsetMap) -> Block {
        Block {
            kind: BlockKind::Paragraph,
            continues: false,
            start,
            end,
            text: String::from(text),
            map,
            runs: Vec::new(),
            align: Align::Auto,
            indent: None,
            space: None,
            size: None,
            image: None,
        }
    }

    #[test]
    fn external_links_have_a_scheme() {
        for href in ["https://example.org/a", "mailto:a@b.c", "HTTP://X"] {
            assert!(is_external(href), "{href}");
        }
        for href in ["text/ch2.xhtml#note", "#note", "c:/x", "../a:b", ""] {
            assert!(!is_external(href), "{href}");
        }
    }

    #[test]
    fn styles_by_run() {
        let mut b = block("plain bold italic", 0, 17, OffsetMap::IDENTITY);
        b.runs = alloc::vec![
            Run {
                start: 6,
                end: 10,
                style: TextStyle::BOLD
            },
            Run {
                start: 11,
                end: 17,
                style: TextStyle::ITALIC
            },
        ];
        assert_eq!(b.style_at(0), TextStyle::REGULAR);
        assert_eq!(b.style_at(6), TextStyle::BOLD);
        assert_eq!(b.style_at(10), TextStyle::REGULAR);
        assert_eq!(b.style_at(16), TextStyle::ITALIC);
        assert!(TextStyle::BOLD.union(TextStyle::ITALIC).italic());
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

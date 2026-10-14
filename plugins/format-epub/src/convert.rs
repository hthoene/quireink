//! Converts an XHTML chapter into QuireInk's compact block format while it
//! is inflated, and stores the result in the plugin's cache:
//!
//! - `<n>.txt`: records of a 6-byte header (kind, heading level, flags,
//!   reserved, text length as u16) followed by the block's UTF-8 text.
//! - `<n>.idx`: the offset of every record's text as u32, for binary
//!   search.
//! - `<n>.anc`: `id<TAB>offset` lines for anchors (table of contents).
//! - `<n>.end`: the length of `<n>.txt` as u32. Written last, so its
//!   presence marks a complete conversion.
//!
//! All files are appended in small pieces while converting, and positions
//! in a chapter are offsets into `<n>.txt`, so neither converting nor
//! reading ever holds more than a block in memory.

use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::document::{BlockKind, DocumentError};
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::zip::{Entry, ZipArchive};

use crate::package::error;

/// Size of a record header.
pub(crate) const HEADER: u32 = 6;
/// Longer paragraphs are split into continued blocks.
const LIMIT: usize = 4000;
/// Output is written to storage in chunks of about this size.
const FLUSH: usize = 1024;
/// Index and anchors are written in chunks of about this size.
const FLUSH_SMALL: usize = 256;

const FLAG_CONTINUES: u8 = 1;

/// Paths of a chapter's cache files.
#[derive(Debug, Clone)]
pub(crate) struct CachePaths {
    pub text: String,
    pub index: String,
    pub anchors: String,
    pub end: String,
}

pub(crate) fn encode_kind(kind: BlockKind) -> (u8, u8) {
    match kind {
        BlockKind::Heading(level) => (1, level),
        BlockKind::Preformatted => (2, 0),
        _ => (0, 0),
    }
}

pub(crate) fn decode_kind(kind: u8, level: u8) -> BlockKind {
    match kind {
        1 => BlockKind::Heading(level.clamp(1, 6)),
        2 => BlockKind::Preformatted,
        _ => BlockKind::Paragraph,
    }
}

pub(crate) fn continues(flags: u8) -> bool {
    flags & FLAG_CONTINUES != 0
}

struct Converter<'a> {
    storage: &'a dyn Storage,
    paths: &'a CachePaths,
    /// Bytes written to `<n>.txt` plus those pending in `out`.
    length: u32,
    out: Vec<u8>,
    index: Vec<u8>,
    anchors: String,
    text: String,
    kind: BlockKind,
    continues: bool,
    /// Depth inside elements whose content is not shown.
    hidden: u32,
    /// Depth inside `<pre>`.
    pre: u32,
    space: bool,
    failed: Option<DocumentError>,
}

impl Converter<'_> {
    /// Where the next character of the current block will be.
    fn position(&self) -> u32 {
        self.length + HEADER + self.text.len() as u32
    }

    fn start(&mut self, name: &str, attrs: quireink_sdk::xml::Attrs<'_>, self_closing: bool) {
        let tag = local_name(name).to_ascii_lowercase();
        if let Some(id) = attrs.get("id") {
            let at = if self.text.is_empty() {
                // The element starts the next block.
                self.length + HEADER
            } else {
                self.position()
            };
            self.anchors.push_str(&alloc::format!("{id}\t{at}\n"));
            if self.anchors.len() >= FLUSH_SMALL {
                self.write_anchors();
            }
        }
        if matches!(tag.as_str(), "head" | "script" | "style" | "title" | "svg") {
            if !self_closing {
                self.hidden += 1;
            }
            return;
        }
        if self.hidden > 0 {
            return;
        }
        match tag.as_str() {
            "br" => {
                if !self.text.is_empty() {
                    self.text.push('\n');
                }
                self.space = false;
            }
            "hr" => {
                self.flush();
                self.text.push_str("* * *");
                self.flush();
            }
            "li" => {
                self.flush();
                self.text.push_str("• ");
            }
            "td" | "th" => self.space = true,
            "pre" => {
                self.flush();
                self.kind = BlockKind::Preformatted;
                self.pre += 1;
            }
            tag if is_block(tag) => {
                self.flush();
                if let Some(level) = heading_level(tag) {
                    self.kind = BlockKind::Heading(level);
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        let tag = local_name(name).to_ascii_lowercase();
        if matches!(tag.as_str(), "head" | "script" | "style" | "title" | "svg") {
            self.hidden = self.hidden.saturating_sub(1);
            return;
        }
        if self.hidden > 0 {
            return;
        }
        if tag == "pre" {
            self.flush();
            self.pre = self.pre.saturating_sub(1);
        } else if tag == "li" || is_block(&tag) {
            self.flush();
        }
    }

    fn characters(&mut self, text: &str) {
        if self.hidden > 0 {
            return;
        }
        for c in decode(text).chars() {
            if self.pre > 0 {
                match c {
                    '\r' => {}
                    '\t' => self.text.push(' '),
                    c => self.text.push(c),
                }
            } else if c == '\u{ad}' {
                // Soft hyphens: dropped until hyphenation uses them.
            } else if c.is_whitespace() && c != '\u{a0}' {
                self.space = true;
            } else {
                if self.space && !self.text.is_empty() && !self.text.ends_with(['\n', ' ']) {
                    self.text.push(' ');
                }
                self.space = false;
                self.text.push(c);
            }
            if self.text.len() >= LIMIT {
                self.split();
            }
        }
    }

    /// Emits the first part of an overlong block at a space.
    fn split(&mut self) {
        let cut = self.text[..LIMIT.min(self.text.len())]
            .rfind([' ', '\n'])
            .filter(|&i| i > 0)
            .unwrap_or_else(|| {
                let mut i = LIMIT.min(self.text.len());
                while !self.text.is_char_boundary(i) {
                    i -= 1;
                }
                i
            });
        let kind = self.kind;
        self.write_record(kind, cut);
        // Keep the rest in place, so the buffer is never reallocated.
        let rest = self.text.len() - self.text.trim_start().len();
        self.text.replace_range(..rest, "");
        self.continues = true;
    }

    /// Ends the current block.
    fn flush(&mut self) {
        let trimmed = self.text.trim_end().len();
        self.text.truncate(trimmed);
        if !self.text.is_empty() {
            let kind = self.kind;
            self.write_record(kind, self.text.len());
        }
        self.text.clear();
        self.kind = BlockKind::Paragraph;
        self.continues = false;
        self.space = false;
    }

    /// Writes the first `len` bytes of the current text as a record and
    /// removes them from the text.
    fn write_record(&mut self, kind: BlockKind, len: usize) {
        let (kind, level) = encode_kind(kind);
        let flags = if self.continues { FLAG_CONTINUES } else { 0 };
        self.index
            .extend_from_slice(&(self.length + HEADER).to_le_bytes());
        if self.index.len() >= FLUSH_SMALL {
            self.write_index();
        }
        self.out.extend_from_slice(&[kind, level, flags, 0]);
        self.out.extend_from_slice(&(len as u16).to_le_bytes());
        if self.out.len() + len > FLUSH {
            // Long blocks go straight to storage instead of growing `out`.
            self.write_out();
            let failed = self.append(&self.paths.text, &self.text.as_bytes()[..len]);
            self.failed = self.failed.take().or(failed);
        } else {
            self.out.extend_from_slice(&self.text.as_bytes()[..len]);
        }
        self.length += HEADER + len as u32;
        self.text.replace_range(..len, "");
    }

    fn append(&self, path: &str, data: &[u8]) -> Option<DocumentError> {
        if data.is_empty() || self.failed.is_some() {
            return None;
        }
        self.storage
            .append(path, data)
            .err()
            .map(DocumentError::Storage)
    }

    fn write_out(&mut self) {
        let failed = self.append(&self.paths.text, &self.out);
        self.failed = self.failed.take().or(failed);
        self.out.clear();
    }

    fn write_index(&mut self) {
        let failed = self.append(&self.paths.index, &self.index);
        self.failed = self.failed.take().or(failed);
        self.index.clear();
    }

    fn write_anchors(&mut self) {
        let failed = self.append(&self.paths.anchors, self.anchors.as_bytes());
        self.failed = self.failed.take().or(failed);
        self.anchors.clear();
    }
}

fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "aside"
            | "nav"
            | "figure"
            | "figcaption"
            | "dt"
            | "dd"
            | "dl"
            | "ul"
            | "ol"
            | "tr"
            | "table"
            | "address"
            | "center"
            | "body"
            | "caption"
    )
}

fn heading_level(tag: &str) -> Option<u8> {
    match tag.as_bytes() {
        [b'h', d @ b'1'..=b'6'] => Some(d - b'0'),
        _ => None,
    }
}

/// Converts a chapter; returns the length of its text file.
pub(crate) fn convert(
    storage: &dyn Storage,
    archive: &ZipArchive,
    entry: &Entry,
    paths: &CachePaths,
) -> Result<u32, DocumentError> {
    let _ = storage.remove(&paths.end);
    for path in [&paths.text, &paths.index, &paths.anchors] {
        storage.write(path, &[])?;
    }
    let mut converter = Converter {
        storage,
        paths,
        length: 0,
        out: Vec::with_capacity(FLUSH + HEADER as usize),
        index: Vec::with_capacity(FLUSH_SMALL + 4),
        anchors: String::new(),
        // Blocks are split before they outgrow this, so it never grows.
        text: String::with_capacity(LIMIT + 4),
        kind: BlockKind::Paragraph,
        continues: false,
        hidden: 0,
        pre: 0,
        space: false,
        failed: None,
    };
    let mut tokenizer = Tokenizer::new();
    let mut handle = |token: Token<'_>| match token {
        Token::Start {
            name,
            attrs,
            self_closing,
        } => {
            converter.start(name, attrs, self_closing);
            if self_closing {
                converter.end(name);
            }
        }
        Token::End { name } => converter.end(name),
        Token::Text(text) => converter.characters(&text),
        Token::CData(text) => converter.characters(text),
    };
    archive
        .stream(storage, entry, &mut |chunk| {
            tokenizer.feed(chunk, &mut handle);
            true
        })
        .map_err(error)?;
    tokenizer.finish(&mut handle);
    converter.flush();
    converter.write_out();
    converter.write_index();
    converter.write_anchors();
    if let Some(e) = converter.failed {
        return Err(e);
    }
    // Last: its presence marks a complete conversion.
    storage.write(&paths.end, &converter.length.to_le_bytes())?;
    Ok(converter.length)
}

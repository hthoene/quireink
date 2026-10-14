//! Converts an XHTML chapter into QuireInk's compact block format while it
//! is inflated, and stores the result in the plugin's cache:
//!
//! - `<n>.txt`: records of an 8-byte header (kind, heading level, flags,
//!   alignment, text length and number of style runs as u16), the block's
//!   UTF-8 text and its style runs (start and end as u16, style as u8).
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
use quireink_sdk::document::{Align, BlockKind, DocumentError, Run, TextStyle};
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::zip::{Entry, ZipArchive};

use crate::css::{self, Props, Rules, parse_align};
use crate::package::error;

/// Size of a record header.
pub(crate) const HEADER: u32 = 8;
/// Size of a style run after a record's text.
pub(crate) const RUN: usize = 5;
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
    /// The book's cache directory.
    pub dir: String,
    pub text: String,
    pub index: String,
    pub anchors: String,
    pub end: String,
}

/// Images are decoded at most this large (longer side, in pixels); readers
/// scale them to the page.
const IMAGE_SIDE: u32 = 1600;
/// Longest side of a decoded cover: enough for the library and home screen.
pub(crate) const COVER_SIDE: u32 = 300;

pub(crate) fn encode_kind(kind: BlockKind) -> (u8, u8) {
    match kind {
        BlockKind::Heading(level) => (1, level),
        BlockKind::Preformatted => (2, 0),
        BlockKind::Image => (3, 0),
        _ => (0, 0),
    }
}

pub(crate) fn decode_kind(kind: u8, level: u8) -> BlockKind {
    match kind {
        1 => BlockKind::Heading(level.clamp(1, 6)),
        2 => BlockKind::Preformatted,
        3 => BlockKind::Image,
        _ => BlockKind::Paragraph,
    }
}

/// The cache file of the image at `archive_path`.
pub(crate) fn image_path(dir: &str, archive_path: &str) -> String {
    let hash = archive_path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    alloc::format!("{dir}/img-{hash:016x}.gray")
}

pub(crate) fn continues(flags: u8) -> bool {
    flags & FLAG_CONTINUES != 0
}

/// An open element: the style and alignment its content inherits.
struct Frame {
    tag: String,
    style: TextStyle,
    align: Align,
    /// The element's content is not shown (`<head>`, `display: none`, …).
    hides: bool,
}

/// What an element sets: defaults for its name, then the stylesheet
/// rules, then its own `align` and `style` attributes (CSS only with
/// `styles`).
fn element_props(
    tag: &str,
    attrs: &quireink_sdk::xml::Attrs<'_>,
    rules: &Rules,
    styles: bool,
) -> Props {
    let mut props = Props::default();
    match tag {
        "b" | "strong" => props.bold = Some(true),
        "i" | "em" | "cite" | "dfn" | "var" => props.italic = Some(true),
        "center" => props.align = Some(Align::Center),
        _ => {}
    }
    let classes = attrs.get("class").unwrap_or_default();
    props.apply(&rules.props(tag, &classes));
    if let Some(value) = attrs.get("align") {
        props.align = parse_align(&value).or(props.align);
    }
    if styles && let Some(css) = attrs.get("style") {
        props.apply(&Props::parse(&css));
    }
    props
}

/// `style` with bold and italic as `props` sets them.
fn styled(style: TextStyle, props: &Props) -> TextStyle {
    let mut bits = style.bits();
    for (flag, on) in [
        (TextStyle::BOLD, props.bold),
        (TextStyle::ITALIC, props.italic),
    ] {
        match on {
            Some(true) => bits |= flag.bits(),
            Some(false) => bits &= !flag.bits(),
            None => {}
        }
    }
    TextStyle::from_bits(bits)
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
    /// Style runs of `text`.
    runs: Vec<Run>,
    /// Alignment of the current block, set by its first character.
    align: Option<Align>,
    /// Open elements.
    stack: Vec<Frame>,
    /// The book's stylesheet rules, plus those of `<style>` elements.
    rules: Rules,
    /// Whether the book's CSS applies.
    styles: bool,
    css: css::Parser,
    /// Directory of the chapter in the archive, for relative links.
    base: String,
    /// Images to decode after the chapter: (archive path, cache path).
    images: Vec<(String, String)>,
    kind: BlockKind,
    continues: bool,
    /// Depth inside elements whose content is not shown.
    hidden: u32,
    /// Depth inside `<pre>`.
    pre: u32,
    space: bool,
    /// Style where the pending space was, e.g. inside a bold word group.
    space_style: TextStyle,
    failed: Option<DocumentError>,
}

impl Converter<'_> {
    fn frame(&self) -> (TextStyle, Align) {
        self.stack
            .last()
            .map_or((TextStyle::REGULAR, Align::Auto), |f| (f.style, f.align))
    }

    /// Appends text in the current style.
    fn push(&mut self, c: char) {
        let (style, _) = self.frame();
        self.push_styled(c, style);
    }

    /// Appends text in `style`.
    fn push_styled(&mut self, c: char, style: TextStyle) {
        let (_, align) = self.frame();
        if self.text.is_empty() {
            self.align.get_or_insert(align);
        }
        let start = self.text.len() as u32;
        self.text.push(c);
        let end = self.text.len() as u32;
        if style == TextStyle::REGULAR {
            return;
        }
        match self.runs.last_mut() {
            Some(run) if run.end == start && run.style == style => run.end = end,
            _ => self.runs.push(Run { start, end, style }),
        }
    }

    fn push_str(&mut self, text: &str) {
        for c in text.chars() {
            self.push(c);
        }
    }

    /// Whether only `<svg>` elements hide the current content.
    fn only_svg_hides(&self) -> bool {
        self.stack
            .iter()
            .filter(|f| f.hides)
            .all(|f| f.tag == "svg")
    }

    /// An image block for `href` (relative to the chapter).
    fn image(&mut self, href: &str, alt: &str) {
        let (path, _) = crate::package::resolve(&self.base, href);
        let cache = image_path(&self.paths.dir, &path);
        self.flush();
        self.kind = BlockKind::Image;
        self.text.push_str(&cache);
        self.text.push('\t');
        self.text.push_str(&clean_alt(alt));
        let len = self.text.len().min(LIMIT);
        self.write_record(BlockKind::Image, len);
        self.text.clear();
        self.runs.clear();
        self.kind = BlockKind::Paragraph;
        if !self.images.iter().any(|(p, _)| *p == path) {
            self.images.push((path, cache));
        }
    }

    /// Removes the first `n` bytes of the text, keeping the runs in step.
    fn drop_front(&mut self, n: usize) {
        self.text.replace_range(..n, "");
        let n = n as u32;
        self.runs.retain_mut(|run| {
            run.start = run.start.saturating_sub(n);
            run.end = run.end.saturating_sub(n);
            run.end > run.start
        });
    }

    /// Where the next character of the current block will be.
    fn position(&self) -> u32 {
        self.length + HEADER + self.text.len() as u32
    }

    fn start(&mut self, name: &str, attrs: quireink_sdk::xml::Attrs<'_>, self_closing: bool) {
        let _ = self_closing;
        let tag = local_name(name).to_ascii_lowercase();
        let props = if self.hidden == 0 {
            element_props(&tag, &attrs, &self.rules, self.styles)
        } else {
            Props::default()
        };
        let hides = matches!(tag.as_str(), "head" | "script" | "style" | "title" | "svg")
            || props.hidden == Some(true);
        // Block elements end the block before them (before their anchor is
        // placed, so it points at the new block), in the old style.
        if !hides
            && self.hidden == 0
            && (is_block(&tag) || matches!(tag.as_str(), "li" | "pre" | "hr"))
        {
            self.flush();
        }
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
        // Self-closing elements are ended right away by the caller.
        let (style, align) = self.frame();
        self.stack.push(Frame {
            tag: tag.clone(),
            style: styled(style, &props),
            align: props.align.unwrap_or(align),
            hides,
        });
        if hides {
            self.hidden += 1;
        }
        // Images, also inside <svg> (a common way to show covers).
        if matches!(tag.as_str(), "img" | "image") && self.only_svg_hides() {
            let href = attrs
                .iter()
                .find(|(name, _)| matches!(local_name(name), "src" | "href"))
                .map(|(_, value)| decode(value).into_owned());
            if let Some(href) = href.filter(|h| !h.is_empty() && !h.starts_with("data:")) {
                let alt = attrs.get("alt").unwrap_or_default();
                self.image(&href, &alt);
            }
            return;
        }
        if self.hidden > 0 {
            return;
        }
        match tag.as_str() {
            "br" => {
                if !self.text.is_empty() {
                    self.push('\n');
                }
                self.space = false;
            }
            "hr" => {
                self.align = Some(Align::Center);
                self.push_str("* * *");
                self.flush();
            }
            "li" => {
                self.push_str("• ");
            }
            "td" | "th" => self.space = true,
            "pre" => {
                self.kind = BlockKind::Preformatted;
                self.pre += 1;
            }
            tag if is_block(tag) => {
                if let Some(level) = heading_level(tag) {
                    self.kind = BlockKind::Heading(level);
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        let tag = local_name(name).to_ascii_lowercase();
        let Some(open) = self.stack.iter().rposition(|f| f.tag == tag) else {
            return;
        };
        if self.hidden == 0 {
            if tag == "pre" {
                self.flush();
                self.pre = self.pre.saturating_sub(1);
            } else if tag == "li" || is_block(&tag) {
                self.flush();
            }
        }
        // Close the element (and any left open inside it).
        for frame in self.stack.drain(open..) {
            if frame.hides {
                self.hidden = self.hidden.saturating_sub(1);
            }
        }
    }

    fn characters(&mut self, text: &str) {
        if self.stack.last().is_some_and(|f| f.tag == "style") {
            if self.styles {
                self.css.feed(text, &mut self.rules);
            }
            return;
        }
        if self.hidden > 0 {
            return;
        }
        for c in decode(text).chars() {
            if self.pre > 0 {
                match c {
                    '\r' => {}
                    '\t' => self.push(' '),
                    c => self.push(c),
                }
            } else if c == '\u{ad}' {
                // Soft hyphens: dropped until hyphenation uses them.
            } else if c.is_whitespace() && c != '\u{a0}' {
                self.space = true;
                self.space_style = self.frame().0;
            } else {
                if self.space && !self.text.is_empty() && !self.text.ends_with(['\n', ' ']) {
                    self.push_styled(' ', self.space_style);
                }
                self.space = false;
                self.push(c);
            }
            if self.text.len() >= LIMIT {
                self.split();
            }
        }
    }

    /// Emits the first part of an overlong block at a space.
    fn split(&mut self) {
        // The last character boundary within the limit.
        let mut limit = LIMIT.min(self.text.len());
        while !self.text.is_char_boundary(limit) {
            limit -= 1;
        }
        let cut = self.text[..limit]
            .rfind([' ', '\n'])
            .filter(|&i| i > 0)
            .unwrap_or(limit);
        let kind = self.kind;
        self.write_record(kind, cut);
        // Keep the rest in place, so the buffer is never reallocated.
        let rest = self.text.len() - self.text.trim_start().len();
        self.drop_front(rest);
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
        self.runs.clear();
        self.align = None;
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
        let runs: Vec<Run> = self
            .runs
            .iter()
            .filter(|run| (run.start as usize) < len)
            .map(|run| Run {
                end: run.end.min(len as u32),
                ..*run
            })
            .collect();
        let align = self.align.unwrap_or_default();
        self.out
            .extend_from_slice(&[kind, level, flags, align.bits()]);
        self.out.extend_from_slice(&(len as u16).to_le_bytes());
        self.out
            .extend_from_slice(&(runs.len() as u16).to_le_bytes());
        if self.out.len() + len > FLUSH {
            // Long blocks go straight to storage instead of growing `out`.
            self.write_out();
            let failed = self.append(&self.paths.text, &self.text.as_bytes()[..len]);
            self.failed = self.failed.take().or(failed);
        } else {
            self.out.extend_from_slice(&self.text.as_bytes()[..len]);
        }
        for run in &runs {
            self.out
                .extend_from_slice(&(run.start as u16).to_le_bytes());
            self.out.extend_from_slice(&(run.end as u16).to_le_bytes());
            self.out.push(run.style.bits());
        }
        self.length += HEADER + len as u32 + (runs.len() * RUN) as u32;
        self.drop_front(len);
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
    rules: Rules,
    styles: bool,
) -> Result<u32, DocumentError> {
    let base = String::from(entry.name.rfind('/').map_or("", |i| &entry.name[..=i]));
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
        runs: Vec::new(),
        align: None,
        stack: Vec::new(),
        rules,
        styles,
        css: css::Parser::default(),
        base,
        images: Vec::new(),
        kind: BlockKind::Paragraph,
        continues: false,
        hidden: 0,
        pre: 0,
        space: false,
        space_style: TextStyle::REGULAR,
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
    // Images after the text, so only one archive entry is inflated at a
    // time. One that cannot be decoded is shown as its alternative text.
    let images = core::mem::take(&mut converter.images);
    let length = converter.length;
    drop(converter);
    for (path, cache) in &images {
        if quireink_sdk::image::GrayFile::open(storage, cache).is_none() {
            let _ = decode_image(storage, archive, path, cache, &paths.dir, IMAGE_SIDE);
        }
    }
    // Last: its presence marks a complete conversion.
    storage.write(&paths.end, &length.to_le_bytes())?;
    Ok(length)
}

/// Collapses whitespace in alternative text.
fn clean_alt(alt: &str) -> String {
    let mut out = String::new();
    for word in alt.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Reads part of a file in storage.
struct StorageSource<'a> {
    storage: &'a dyn Storage,
    path: &'a str,
    offset: u64,
    end: u64,
}

impl quireink_sdk::image::Source for StorageSource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, quireink_sdk::image::ImageError> {
        let left = (self.end - self.offset).min(buf.len() as u64) as usize;
        if left == 0 {
            return Ok(0);
        }
        let n = self
            .storage
            .read_at(self.path, self.offset, &mut buf[..left])
            .map_err(|_| quireink_sdk::image::ImageError::Io)?;
        self.offset += n as u64;
        Ok(n)
    }
}

/// Decodes the image at `path` in the archive into the cache file `cache`.
/// Stored entries are read in place; compressed ones are inflated to a
/// temporary file first.
pub(crate) fn decode_image(
    storage: &dyn Storage,
    archive: &ZipArchive,
    path: &str,
    cache: &str,
    dir: &str,
    side: u32,
) -> Result<(), DocumentError> {
    use quireink_sdk::image::{Format, GrayWriter, decode};

    let entry = archive
        .entry(storage, path)
        .map_err(error)?
        .ok_or(DocumentError::Malformed(alloc::borrow::Cow::Borrowed(
            "missing image",
        )))?;
    let temp = alloc::format!("{dir}/image.tmp");
    let (file, offset, end) = match archive.stored_range(storage, &entry).map_err(error)? {
        Some((offset, len)) => (archive.path(), offset, offset + len),
        None => {
            storage.write(&temp, &[])?;
            let mut failed = None;
            archive
                .stream(storage, &entry, &mut |chunk| {
                    failed = storage.append(&temp, chunk).err();
                    failed.is_none()
                })
                .map_err(error)?;
            if let Some(e) = failed {
                return Err(DocumentError::Storage(e));
            }
            (temp.as_str(), 0, u64::from(entry.size))
        }
    };
    let mut head = [0u8; 8];
    let n = storage.read_at(file, offset, &mut head)?;
    let result = match Format::detect(&head[..n]) {
        Some(format) => {
            let mut source = StorageSource {
                storage,
                path: file,
                offset,
                end,
            };
            let mut writer = GrayWriter::new(storage, cache);
            decode(format, &mut source, side, &mut writer).and_then(|()| writer.finish())
        }
        None => Err(quireink_sdk::image::ImageError::Unsupported("image format")),
    };
    if file == temp {
        let _ = storage.remove(&temp);
    }
    if result.is_err() {
        let _ = storage.remove(cache);
    }
    result.map_err(|e| DocumentError::Malformed(alloc::borrow::Cow::Owned(alloc::format!("{e}"))))
}

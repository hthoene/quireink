//! Converts an XHTML chapter into QuireInk's compact block format while it
//! is inflated, and stores the result in the plugin's cache:
//!
//! - `<n>.txt`: records of an 8-byte header (kind, heading level, flags,
//!   alignment, text length and number of style runs as u16), the block's
//!   UTF-8 text and its style runs (start and end as u16, style as u8).
//! - `<n>.idx`: for every record, the offset of its text as u32 (for
//!   binary search), the book's indent as i16 and the book's space above
//!   as u16, in hundredths of an em ([`NO_INDENT`] and [`NO_SPACE`] if it
//!   sets none), and its text size as u16 in percent of body text (0 if it
//!   sets none). Kept apart from the text so positions in it stay the same
//!   when more is stored per block.
//! - `<n>.anc`: `id<TAB>offset` lines for anchors (table of contents).
//! - `<n>.lnk`: `start<TAB>end<TAB>href` lines for links, in order: `href`
//!   as an archive path with its fragment (`text/ch2.xhtml#note3`), or as
//!   written for links out of the book.
//! - `<n>.pos`: `offset<TAB>path` lines: the element each record comes
//!   from, as a path below `<body>` such as `div[2]/p[22]`, for positions
//!   other readers understand (KOReader's XPointers).
//! - `<n>.end`: the length of `<n>.txt` as u32. Written last, so its
//!   presence marks a complete conversion.
//!
//! All files are appended in small pieces while converting, and positions
//! in a chapter are offsets into `<n>.txt`, so neither converting nor
//! reading ever holds more than a block in memory.
//!
//! HTML files (e.g. web pages) are converted the same way, read as
//! browsers do (see [`Source::Html`]).

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
/// Bytes per record in `<n>.idx`.
pub(crate) const INDEX: u32 = 10;
/// A record without the book's indent or space.
pub(crate) const NO_INDENT: i16 = i16::MIN;
pub(crate) const NO_SPACE: u16 = u16::MAX;
/// Size of a style run after a record's text.
pub(crate) const RUN: usize = 5;
/// Longer paragraphs are split into continued blocks.
const LIMIT: usize = 4000;
/// Output is written to storage in chunks of about this size.
const FLUSH: usize = 1024;
/// Index and anchors are written in chunks of about this size.
const FLUSH_SMALL: usize = 256;

const FLAG_CONTINUES: u8 = 1;

/// Where a chapter's markup comes from.
pub(crate) enum Source<'a> {
    /// A chapter of an EPUB.
    Zip {
        archive: &'a ZipArchive,
        entry: &'a Entry,
    },
    /// An HTML file in storage. `<script>` and `<style>` hold raw text,
    /// elements may be left open, and only the reading matter is shown:
    /// the `focus` element (`main` or `article`) if the page has one, and
    /// never navigation, forms and the like. Links are made absolute with
    /// `base` (the page's address), else left relative to the file.
    Html {
        path: &'a str,
        focus: Option<&'a str>,
        base: Option<&'a str>,
    },
}

/// Paths of a chapter's cache files.
#[derive(Debug, Clone)]
pub(crate) struct CachePaths {
    /// The book's cache directory.
    pub dir: String,
    pub text: String,
    pub index: String,
    pub anchors: String,
    pub links: String,
    pub positions: String,
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

/// The larger of two spaces, as collapsing margins are.
fn larger(a: Option<u16>, b: Option<u16>) -> Option<u16> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

pub(crate) fn continues(flags: u8) -> bool {
    flags & FLAG_CONTINUES != 0
}

/// An open element: the style and alignment its content inherits.
struct Frame {
    tag: String,
    style: TextStyle,
    align: Align,
    /// The book's indent, inherited as in CSS.
    indent: Option<i16>,
    /// The book's text size in percent of body text, inherited.
    size: Option<u16>,
    /// The margin and padding below the element, taken over by the next
    /// block.
    after: (Option<u16>, Option<u16>),
    /// The element's content is not shown (`<head>`, `display: none`, …).
    hides: bool,
    /// Its number among its parent's children of the same name (1 for
    /// the first).
    index: u32,
    /// A link (`<a href>`): where it starts, and where it goes.
    link: Option<(u32, String)>,
    /// The reading matter of an HTML page: its content is shown.
    focus: bool,
    /// How many children of each name it has had so far.
    children: Vec<(String, u32)>,
}

/// What an element sets: defaults for its name, then the stylesheet
/// rules, then its own `align` and `style` attributes (CSS only with
/// `styles`).
fn element_props(
    tag: &str,
    attrs: &quireink_sdk::xml::Attrs<'_>,
    rules: &mut Rules,
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
        props.apply(&rules.parse_style(&css));
    }
    props
}

/// `style` with bold, italic and the font family as `props` sets them.
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
    let style = TextStyle::from_bits(bits);
    match props.family {
        Some(family) => style.with_family(family),
        None => style,
    }
}

/// How an HTML page is read.
struct Html {
    /// The element holding the reading matter.
    focus: Option<String>,
    /// The page's address, for absolute links.
    base: Option<String>,
}

struct Converter<'a> {
    storage: &'a dyn Storage,
    paths: &'a CachePaths,
    /// Bytes written to `<n>.txt` plus those pending in `out`.
    length: u32,
    out: Vec<u8>,
    index: Vec<u8>,
    anchors: String,
    /// Pending `<n>.lnk` lines.
    links: String,
    /// The chapter's archive path, for links within it.
    chapter: String,
    /// Reading HTML: the page's address, if known (else `None` for
    /// EPUB chapters and HTML without one).
    html: Option<Html>,
    /// Pending `<n>.pos` lines.
    positions: String,
    /// The element path of the block being collected.
    block_path: String,
    text: String,
    /// Style runs of `text`.
    runs: Vec<Run>,
    /// Alignment of the current block, set by its first character.
    align: Option<Align>,
    /// The book's indent of the current block, set by its first character.
    indent: Option<i16>,
    /// The book's text size of the current block, set likewise.
    size: Option<u16>,
    /// The book's space above the next block, as in CSS: the largest of
    /// the margins met since the last block (margins collapse) plus all
    /// paddings.
    margin: Option<u16>,
    padding: Option<u16>,
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
            self.indent = self.stack.last().and_then(|f| f.indent);
            self.size = self.stack.last().and_then(|f| f.size);
            self.block_path = self.element_path();
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

    /// Where an element starting now begins: after the space still to
    /// come, or at the next block if none is being collected.
    fn here(&self) -> u32 {
        if self.text.is_empty() {
            self.length + HEADER
        } else {
            self.position() + u32::from(self.space)
        }
    }

    /// Where a link goes, as stored in `<n>.lnk`; `None` for none.
    fn target(&self, href: &str) -> Option<String> {
        let href: String = href.trim().chars().filter(|c| !c.is_control()).collect();
        if href.is_empty() || quireink_sdk::document::is_external(&href) {
            return (!href.is_empty()).then_some(href);
        }
        if let Some(html) = &self.html {
            return Some(match (href.strip_prefix('#'), &html.base) {
                (Some(_), _) => href,
                (None, Some(base)) => quireink_sdk::web::join_url(base, &href),
                (None, None) => crate::package::resolve(&self.base, &href).0,
            });
        }
        let (path, fragment) = if let Some(fragment) = href.strip_prefix('#') {
            (self.chapter.clone(), Some(String::from(fragment)))
        } else {
            crate::package::resolve(&self.base, &href)
        };
        Some(match fragment {
            Some(fragment) => alloc::format!("{path}#{fragment}"),
            None => path,
        })
    }

    fn start(&mut self, name: &str, attrs: quireink_sdk::xml::Attrs<'_>, self_closing: bool) {
        let _ = self_closing;
        let tag = local_name(name).to_ascii_lowercase();
        let props = if self.hidden == 0 {
            element_props(&tag, &attrs, &mut self.rules, self.styles)
        } else {
            Props::default()
        };
        let hides = matches!(tag.as_str(), "head" | "script" | "style" | "title" | "svg")
            || props.hidden == Some(true)
            || self
                .html
                .as_ref()
                .is_some_and(|html| furniture(&tag, &attrs, html.focus.is_some()));
        // HTML leaves paragraphs and list items open: the next one ends
        // them.
        if self.html.is_some() {
            let open = self.stack.last().map(|f| f.tag.as_str());
            let closes = match tag.as_str() {
                "li" => matches!(open, Some("li" | "p")),
                "dt" | "dd" => matches!(open, Some("dt" | "dd" | "p")),
                "td" | "th" => matches!(open, Some("td" | "th")),
                "tr" => matches!(open, Some("tr" | "td" | "th")),
                tag => is_block(tag) && open == Some("p"),
            };
            if closes && let Some(open) = open.map(String::from) {
                self.end(&open);
            }
        }
        // Block elements end the block before them (before their anchor is
        // placed, so it points at the new block), in the old style.
        if !hides
            && self.hidden == 0
            && (is_block(&tag) || matches!(tag.as_str(), "li" | "pre" | "hr"))
        {
            self.flush();
        }
        if let Some(id) = attrs.get("id") {
            let at = self.here();
            self.anchors.push_str(&alloc::format!("{id}\t{at}\n"));
            if self.anchors.len() >= FLUSH_SMALL {
                self.write_anchors();
            }
            if self.links.len() >= FLUSH_SMALL {
                self.write_links();
            }
        }
        // Self-closing elements are ended right away by the caller.
        let (style, align) = self.frame();
        if self.text.is_empty() {
            self.margin = larger(self.margin, props.margin_top);
            self.padding = css::sum(self.padding, props.padding_top);
        }
        let indent = props
            .indent
            .or_else(|| self.stack.last().and_then(|f| f.indent));
        let parent_size = self.stack.last().and_then(|f| f.size);
        let size = match props.size {
            Some(size) => Some(size.of(parent_size.unwrap_or(100))),
            None => parent_size,
        };
        // Its number among its parent's children of the same name.
        let index = match self.stack.last_mut() {
            Some(parent) => match parent.children.iter_mut().find(|(t, _)| *t == tag) {
                Some((_, count)) => {
                    *count += 1;
                    *count
                }
                None => {
                    parent.children.push((tag.clone(), 1));
                    1
                }
            },
            None => 1,
        };
        let link = match attrs.get("href") {
            Some(href) if tag == "a" && !hides && self.hidden == 0 => {
                self.target(&href).map(|target| (self.here(), target))
            }
            _ => None,
        };
        let focus = self
            .html
            .as_ref()
            .and_then(|html| html.focus.as_deref())
            .is_some_and(|focus| focus == tag && !self.stack.iter().any(|f| f.focus));
        if focus {
            // The reading matter begins: shown, unless hidden otherwise.
            self.hidden = self.hidden.saturating_sub(1);
        }
        self.stack.push(Frame {
            index,
            link,
            focus,
            children: Vec::new(),
            tag: tag.clone(),
            style: styled(style, &props),
            align: props.align.unwrap_or(align),
            indent,
            size,
            after: (props.margin_bottom, props.padding_bottom),
            hides,
        });
        if hides {
            self.hidden += 1;
        }
        // Images, also inside <svg> (a common way to show covers).
        if matches!(tag.as_str(), "img" | "image") && self.html.is_none() && self.only_svg_hides() {
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
        let (mut margin, mut padding) = (None, None);
        let here = self.position();
        for frame in self.stack.drain(open..) {
            if frame.hides {
                self.hidden = self.hidden.saturating_sub(1);
            }
            if frame.focus {
                self.hidden += 1;
            }
            if let Some((start, href)) = frame.link
                && here > start
            {
                self.links
                    .push_str(&alloc::format!("{start}\t{here}\t{href}\n"));
            }
            margin = larger(margin, frame.after.0);
            padding = css::sum(padding, frame.after.1);
        }
        if self.text.is_empty() {
            self.margin = larger(self.margin, margin);
            self.padding = css::sum(self.padding, padding);
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
        self.indent = None;
        self.size = None;
        self.kind = BlockKind::Paragraph;
        self.continues = false;
        self.space = false;
    }

    /// Writes the first `len` bytes of the current text as a record and
    /// removes them from the text.
    fn write_record(&mut self, kind: BlockKind, len: usize) {
        let (kind, level) = encode_kind(kind);
        let flags = if self.continues { FLAG_CONTINUES } else { 0 };
        self.positions.push_str(&alloc::format!(
            "{}\t{}\n",
            self.length + HEADER,
            self.block_path
        ));
        if self.positions.len() >= FLUSH_SMALL {
            self.write_positions();
        }
        let indent = self.indent.unwrap_or(NO_INDENT);
        let space = if self.continues {
            NO_SPACE
        } else {
            let (margin, padding) = (self.margin.take(), self.padding.take());
            css::sum(margin, padding).unwrap_or(NO_SPACE)
        };
        self.index
            .extend_from_slice(&(self.length + HEADER).to_le_bytes());
        self.index.extend_from_slice(&indent.to_le_bytes());
        self.index.extend_from_slice(&space.to_le_bytes());
        let size = self.size.filter(|&s| s != 100).unwrap_or(0);
        self.index.extend_from_slice(&size.to_le_bytes());
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

    /// The path of the innermost open element below `<body>`, e.g.
    /// `div[2]/p[22]` (indexes count siblings of the same name).
    fn element_path(&self) -> String {
        let body = self.stack.iter().position(|f| f.tag == "body");
        let mut path = String::new();
        for frame in body.map_or(&[][..], |b| &self.stack[b + 1..]) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(&alloc::format!("{}[{}]", frame.tag, frame.index));
        }
        path
    }

    fn write_positions(&mut self) {
        let failed = self.append(&self.paths.positions, self.positions.as_bytes());
        self.failed = self.failed.take().or(failed);
        self.positions.clear();
    }

    fn write_anchors(&mut self) {
        let failed = self.append(&self.paths.anchors, self.anchors.as_bytes());
        self.failed = self.failed.take().or(failed);
        self.anchors.clear();
    }

    fn write_links(&mut self) {
        let failed = self.append(&self.paths.links, self.links.as_bytes());
        self.failed = self.failed.take().or(failed);
        self.links.clear();
    }
}

/// Elements without content or end tag in HTML.
fn is_void(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Words in class names and ids of page furniture (`site-nav`,
/// `vector-dropdown`, `mw-editsection`, …).
const FURNITURE: [&str; 24] = [
    "nav",
    "navbar",
    "navigation",
    "navbox",
    "menu",
    "dropdown",
    "sidebar",
    "banner",
    "cookie",
    "cookies",
    "share",
    "sharing",
    "social",
    "related",
    "comments",
    "advert",
    "ads",
    "promo",
    "popup",
    "modal",
    "noprint",
    "breadcrumb",
    "breadcrumbs",
    "editsection",
];

/// Parts of a web page that are not reading matter: navigation, forms,
/// menus and the like, by element, ARIA role or class name. Headers and
/// footers belong to the reading matter when it is marked (an article's
/// title), else they are the site's.
fn furniture(tag: &str, attrs: &quireink_sdk::xml::Attrs<'_>, focus: bool) -> bool {
    let role = attrs.get("role");
    if matches!(
        role.as_deref(),
        Some("navigation" | "search" | "banner" | "contentinfo" | "complementary" | "menu")
    ) || attrs.get("aria-hidden").as_deref() == Some("true")
    {
        return true;
    }
    // A class or id naming furniture starts or ends with its word
    // (`nav-links`, `site-nav`); words in between describe a state
    // (`main-menu-pinned-disabled` on `<body>`).
    let furniture_word = |word: &str| FURNITURE.contains(&word.to_ascii_lowercase().as_str());
    let named = |name: &str| {
        attrs.get(name).is_some_and(|value| {
            value.split_whitespace().any(|token| {
                let mut words = token.split(['-', '_']).filter(|w| !w.is_empty());
                let first = words.next();
                let last = words.next_back();
                first.is_some_and(furniture_word) || last.is_some_and(furniture_word)
            })
        })
    };
    let frame = matches!(tag, "html" | "body" | "main" | "article");
    if !frame && (named("class") || named("id")) {
        return true;
    }
    matches!(
        tag,
        "nav"
            | "aside"
            | "form"
            | "noscript"
            | "iframe"
            | "button"
            | "select"
            | "textarea"
            | "template"
            | "dialog"
            | "menu"
    ) || (!focus && matches!(tag, "header" | "footer"))
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
    source: &Source<'_>,
    paths: &CachePaths,
    rules: Rules,
    styles: bool,
) -> Result<u32, DocumentError> {
    let name = match source {
        Source::Zip { entry, .. } => entry.name.as_str(),
        Source::Html { path, .. } => path,
    };
    let base = String::from(name.rfind('/').map_or("", |i| &name[..=i]));
    let html = match source {
        Source::Zip { .. } => None,
        Source::Html { focus, base, .. } => Some(Html {
            focus: focus.map(String::from),
            base: base.map(String::from),
        }),
    };
    let _ = storage.remove(&paths.end);
    for path in [
        &paths.text,
        &paths.index,
        &paths.anchors,
        &paths.links,
        &paths.positions,
    ] {
        storage.write(path, &[])?;
    }
    let mut converter = Converter {
        storage,
        paths,
        length: 0,
        out: Vec::with_capacity(FLUSH + HEADER as usize),
        index: Vec::with_capacity(FLUSH_SMALL + 4),
        anchors: String::new(),
        links: String::new(),
        chapter: String::from(name),
        positions: String::new(),
        block_path: String::new(),
        // Blocks are split before they outgrow this, so it never grows.
        text: String::with_capacity(LIMIT + 4),
        runs: Vec::new(),
        align: None,
        indent: None,
        size: None,
        margin: None,
        padding: None,
        stack: Vec::new(),
        rules,
        styles,
        css: css::Parser::new(&base),
        base,
        images: Vec::new(),
        kind: BlockKind::Paragraph,
        continues: false,
        // A page with reading matter shows nothing outside it.
        hidden: u32::from(html.as_ref().is_some_and(|h| h.focus.is_some())),
        html,
        pre: 0,
        space: false,
        space_style: TextStyle::REGULAR,
        failed: None,
    };
    let mut tokenizer = match source {
        Source::Zip { .. } => Tokenizer::new(),
        Source::Html { .. } => Tokenizer::html(),
    };
    let mut handle = |token: Token<'_>| match token {
        Token::Start {
            name,
            attrs,
            self_closing,
        } => {
            converter.start(name, attrs, self_closing);
            // Empty elements, also as HTML writes them (`<br>`).
            if self_closing || is_void(local_name(name)) {
                converter.end(name);
            }
        }
        Token::End { name } => converter.end(name),
        Token::Text(text) => converter.characters(&text),
        Token::CData(text) => converter.characters(text),
    };
    match source {
        Source::Zip { archive, entry } => archive
            .stream(storage, entry, &mut |chunk| {
                tokenizer.feed(chunk, &mut handle);
                true
            })
            .map_err(error)?,
        Source::Html { path, .. } => {
            let mut chunk = [0u8; 1024];
            let mut at = 0u64;
            loop {
                let n = storage.read_at(path, at, &mut chunk)?;
                if n == 0 {
                    break;
                }
                tokenizer.feed(&chunk[..n], &mut handle);
                at += n as u64;
            }
        }
    }
    tokenizer.finish(&mut handle);
    converter.flush();
    converter.write_out();
    converter.write_index();
    converter.write_anchors();
    converter.write_links();
    converter.write_positions();
    if let Some(e) = converter.failed {
        return Err(e);
    }
    // Images after the text, so only one archive entry is inflated at a
    // time. One that cannot be decoded is shown as its alternative text.
    let images = core::mem::take(&mut converter.images);
    let length = converter.length;
    drop(converter);
    if let Source::Zip { archive, .. } = source {
        for (path, cache) in &images {
            if quireink_sdk::image::GrayFile::open(storage, cache).is_none() {
                let _ = decode_image(storage, archive, path, cache, &paths.dir, IMAGE_SIDE);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::xml::{Token, Tokenizer};

    fn is_furniture(tag: &str) -> bool {
        let mut found = None;
        Tokenizer::parse(tag.as_bytes(), &mut |token| {
            if let Token::Start { name, attrs, .. } = token {
                found = Some(furniture(name, &attrs, true));
            }
        });
        found.unwrap()
    }

    #[test]
    fn page_furniture_is_recognized() {
        for tag in [
            "<nav>",
            "<div role=\"navigation\">",
            "<div class=\"vector-dropdown mw-portlet\">",
            "<span class=\"mw-editsection\">",
            "<div id=\"site_sidebar\">",
            "<p aria-hidden=\"true\">",
            "<div class=\"NoPrint\">",
        ] {
            assert!(is_furniture(tag), "{tag}");
        }
        for tag in [
            "<p>",
            "<header>",
            "<div class=\"unavailable canvas\">",
            "<div class=\"article-header\">",
            "<div id=\"mw-content-text\">",
            "<div class=\"vector-feature-main-menu-pinned-disabled\">",
            "<body class=\"menu-open\">",
        ] {
            assert!(!is_furniture(tag), "{tag}");
        }
    }
}

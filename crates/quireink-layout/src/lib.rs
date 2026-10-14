//! Page layout: turns a [`Document`] into pages.
//!
//! Layout is deterministic and starts at any [`Locator`] that begins a line,
//! so a reader only needs to remember where pages start — never the pages
//! themselves. Text is broken greedily per paragraph; because greedy breaking
//! is stable, laying out from the middle of a paragraph yields exactly the
//! same lines as laying out the whole paragraph.
//!
//! The engine knows nothing about fonts or drawing: a [`Typesetter`]
//! measures characters in each [`Face`] (character style, body text or
//! heading level), and the engine returns positioned lines with their runs
//! of faces.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub use quireink_document::ImageRef;
pub use quireink_document::TextStyle;
use quireink_document::{Align, Block, BlockKind, Document, DocumentError, Locator, Storage};
pub use quireink_text::Hyphenate;
use quireink_text::wrap_lines_hyphenated;

/// Largest extra space per word gap for justification, relative to the
/// line height.
const MAX_JUSTIFY_GAP: f32 = 0.6;
/// Extra space above a heading, relative to a body line.
const HEADING_SPACE: f32 = 0.6;

/// How text is set: its character style and whether it is body text or a
/// heading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Face {
    pub style: TextStyle,
    /// 0 for body text, 1 (most important) to 6 for headings.
    pub heading: u8,
}

/// Measures text for the layout engine; readers implement it with their
/// fonts.
pub trait Typesetter {
    /// Advance width of `c` in `face`.
    fn advance(&self, face: Face, c: char) -> f32;

    /// Lines in `face`: `(ascent, line height)`, i.e. the distance from
    /// the top of a line box to its baseline and between baselines.
    fn line(&self, face: Face) -> (f32, f32);
}

/// Typographic settings for a page, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Extra space between paragraphs.
    pub paragraph_spacing: f32,
    /// Indent of the first line of a paragraph.
    pub indent: f32,
    /// Stretch spaces so lines fill the width (except a paragraph's last),
    /// unless a block asks for another alignment.
    pub justify: bool,
}

/// Text of a line set in one face: byte range of [`PageLine::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRun {
    pub start: usize,
    pub end: usize,
    pub face: Face,
}

/// A line of text at its position on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLine {
    /// Left edge relative to the content box.
    pub x: f32,
    /// Baseline relative to the content box's top.
    pub baseline: f32,
    pub text: String,
    /// Extra width to add to every space (justification).
    pub word_spacing: f32,
    /// The faces of `text`, in order and covering all of it.
    pub runs: Vec<LineRun>,
}

/// An image at its position and size on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageImage {
    /// Left and top edges relative to the content box.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub image: ImageRef,
}

/// One laid-out page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    /// Position of the first character on the page.
    pub start: Locator,
    /// Position where the next page starts; equal to `start` past the end.
    pub end: Locator,
    pub lines: Vec<PageLine>,
    pub images: Vec<PageImage>,
}

impl Page {
    /// Whether this page is past the end of the document.
    pub fn is_past_end(&self) -> bool {
        self.lines.is_empty() && self.images.is_empty()
    }
}

/// The face of a block's text at byte `index`.
fn face_at(block: &Block, base: Face, index: usize) -> Face {
    Face {
        style: base.style.union(block.style_at(index)),
        heading: base.heading,
    }
}

/// The runs of faces for `range` of the block's text, as ranges of a line
/// text starting at `range.start`, plus `extra` bytes at the end (a hyphen).
fn line_runs(
    block: &Block,
    base: Face,
    range: core::ops::Range<usize>,
    extra: usize,
) -> Vec<LineRun> {
    let mut runs: Vec<LineRun> = Vec::new();
    for (i, _) in block.text[range.clone()].char_indices() {
        let face = face_at(block, base, range.start + i);
        match runs.last_mut() {
            Some(run) if run.face == face => run.end = i,
            _ => runs.push(LineRun {
                start: i,
                end: i,
                face,
            }),
        }
    }
    // Ends are exclusive: every run reaches the next one or the line end.
    let len = range.len() + extra;
    for k in 0..runs.len() {
        runs[k].end = runs.get(k + 1).map_or(len, |next| next.start);
    }
    if runs.is_empty() && len > 0 {
        runs.push(LineRun {
            start: 0,
            end: len,
            face: base,
        });
    }
    runs
}

/// Lays out the page starting at `start` into a `width`×`height` content
/// box, measuring text with `typesetter`; with `hyphenate`, words that do
/// not fit at the end of a line are hyphenated.
///
/// Headings are set in their own face, never indented or justified, and
/// never left alone at the bottom of a page.
#[allow(clippy::too_many_arguments)]
pub fn layout_page(
    document: &dyn Document,
    storage: &dyn Storage,
    start: Locator,
    width: f32,
    height: f32,
    style: &Style,
    typesetter: &dyn Typesetter,
    hyphenate: Option<Hyphenate<'_>>,
) -> Result<Page, DocumentError> {
    let mut page = Page {
        start,
        end: start,
        lines: Vec::new(),
        images: Vec::new(),
    };
    let mut at = start;
    let mut y = 0.0f32;
    let (_, body_line) = typesetter.line(Face::default());

    loop {
        let Some(block) = document.block_at(storage, at)? else {
            // End of the section: chapters start on a new page.
            if at.section + 1 < document.sections() && page.is_past_end() {
                at = Locator::new(at.section + 1, 0);
                page.start = at;
                continue;
            }
            if at.section + 1 < document.sections() {
                at = Locator::new(at.section + 1, 0);
            }
            break;
        };
        let from = if at.offset > block.start {
            block.text_index(at.offset)
        } else {
            0
        };
        if page.is_past_end() && from == 0 && at.offset < block.start {
            page.start = Locator::new(at.section, block.start);
        }
        if block.kind == BlockKind::Image
            && let Some(image) = &block.image
        {
            // Scaled to fit the content box, keeping its proportions.
            let scale = (width / image.width as f32).min(height / image.height as f32);
            let (w, h) = (image.width as f32 * scale, image.height as f32 * scale);
            let spacing = if page.is_past_end() {
                0.0
            } else {
                style.paragraph_spacing.max(body_line * 0.5)
            };
            if !page.is_past_end() && y + spacing + h > height {
                page.end = Locator::new(at.section, block.start);
                return Ok(page);
            }
            if page.is_past_end() {
                page.start = Locator::new(at.section, block.start);
            }
            y += spacing;
            page.images.push(PageImage {
                x: (width - w) / 2.0,
                y,
                width: w,
                height: h,
                image: image.clone(),
            });
            y += h + spacing;
            at = Locator::new(at.section, block.end);
            continue;
        }
        let heading = match block.kind {
            BlockKind::Heading(level) => level.clamp(1, 6),
            _ => 0,
        };
        let base = Face {
            style: if heading > 0 {
                TextStyle::BOLD
            } else {
                TextStyle::REGULAR
            },
            heading,
        };
        let (ascent, line_height) = typesetter.line(base);
        let lines_before = page.lines.len();
        let had_content = !page.is_past_end();
        let at_block_start = from == 0 && !block.continues;
        if !page.is_past_end() && at_block_start {
            y += style.paragraph_spacing;
            if heading > 0 {
                y += body_line * HEADING_SPACE;
            }
        }
        let align = match block.align {
            Align::Auto if heading > 0 => Align::Start,
            Align::Auto if style.justify => Align::Justify,
            Align::Auto => Align::Start,
            align => align,
        };
        let indent = if at_block_start
            && block.kind == BlockKind::Paragraph
            && matches!(align, Align::Start | Align::Justify)
        {
            style.indent
        } else {
            0.0
        };
        let text = &block.text[from..];
        let lines = wrap_lines_hyphenated(
            text,
            |n| if n == 0 { width - indent } else { width },
            |i, c| typesetter.advance(face_at(&block, base, from + i), c),
            hyphenate,
        );

        for (n, line) in lines.iter().enumerate() {
            let fits = y + line_height <= height;
            if !fits && !page.is_past_end() {
                page.end = Locator::new(at.section, block.source_offset(from + line.range.start));
                return Ok(page);
            }
            let first_x = if n == 0 { indent } else { 0.0 };
            let available = if n == 0 { width - indent } else { width };
            let free = (available - line.width).max(0.0);
            let (x, word_spacing) = match align {
                Align::Center => (first_x + free / 2.0, 0.0),
                Align::End => (first_x + free, 0.0),
                Align::Justify if !line.last && line.spaces > 0 => {
                    let extra = free / line.spaces as f32;
                    // Huge gaps look worse than a ragged line: set those
                    // lines flush left (e.g. a few words before a very long
                    // word).
                    if extra > line_height * MAX_JUSTIFY_GAP {
                        (first_x, 0.0)
                    } else {
                        (first_x, extra)
                    }
                }
                _ => (first_x, 0.0),
            };
            let mut line_text = String::from(&text[line.range.clone()]);
            if line.hyphen {
                line_text.push('-');
            }
            let range = from + line.range.start..from + line.range.end;
            page.lines.push(PageLine {
                x,
                baseline: y + ascent,
                text: line_text,
                word_spacing,
                runs: line_runs(&block, base, range, usize::from(line.hyphen)),
            });
            y += line_height;
        }
        // A heading needs at least one line of what follows on its page.
        if heading > 0 && had_content && y + body_line > height {
            page.lines.truncate(lines_before);
            page.end = Locator::new(at.section, block.source_offset(from));
            return Ok(page);
        }
        at = Locator::new(at.section, block.end);
    }
    page.end = at;
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use quireink_document::{Metadata, OffsetMap, Run};

    /// Paragraphs separated by single newlines; offsets are bytes. Lines
    /// starting with `#` are headings; `*word*` is bold.
    struct Text {
        text: &'static str,
        metadata: Metadata,
    }

    impl Text {
        fn new(text: &'static str) -> Self {
            Self {
                text,
                metadata: Metadata::default(),
            }
        }
    }

    impl Document for Text {
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        fn sections(&self) -> u32 {
            1
        }

        fn section_len(&self, _: u32) -> u32 {
            self.text.len() as u32
        }

        fn block_at(&self, _: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError> {
            let offset = at.offset as usize;
            if offset >= self.text.len() {
                return Ok(None);
            }
            let start = self.text[..offset].rfind('\n').map_or(0, |i| i + 1);
            let end = self.text[offset..]
                .find('\n')
                .map_or(self.text.len(), |i| offset + i + 1);
            let line = self.text[start..end].trim_end_matches('\n');
            let runs = line
                .match_indices('*')
                .collect::<Vec<_>>()
                .chunks(2)
                .filter(|pair| pair.len() == 2)
                .map(|pair| Run {
                    start: pair[0].0 as u32,
                    end: pair[1].0 as u32 + 1,
                    style: TextStyle::BOLD,
                })
                .collect();
            Ok(Some(Block {
                kind: if line.starts_with('#') {
                    BlockKind::Heading(1)
                } else {
                    BlockKind::Paragraph
                },
                continues: false,
                start: start as u32,
                end: end as u32,
                text: String::from(line),
                map: OffsetMap::IDENTITY,
                runs,
                align: if line.starts_with('>') {
                    Align::Center
                } else {
                    Align::Auto
                },
                image: None,
            }))
        }
    }

    /// Body text: 1 unit per character (bold 2), lines of 10 with ascent 8;
    /// headings: 2 units, lines of 20.
    struct Units;

    impl Typesetter for Units {
        fn advance(&self, face: Face, _: char) -> f32 {
            if face.heading > 0 || face.style.bold() {
                2.0
            } else {
                1.0
            }
        }

        fn line(&self, face: Face) -> (f32, f32) {
            if face.heading > 0 {
                (16.0, 20.0)
            } else {
                (8.0, 10.0)
            }
        }
    }

    const STYLE: Style = Style {
        paragraph_spacing: 0.0,
        indent: 0.0,
        justify: false,
    };

    fn texts(page: &Page) -> Vec<&str> {
        page.lines.iter().map(|l| l.text.as_str()).collect()
    }

    fn layout(doc: &Text, start: u32, height: f32, style: &Style) -> Page {
        let storage = quireink_document::NoStorage;
        layout_page(
            doc,
            &storage,
            Locator::new(0, start),
            10.0,
            height,
            style,
            &Units,
            None,
        )
        .unwrap()
    }

    #[test]
    fn fills_pages_and_continues_mid_paragraph() {
        let doc = Text::new("aaaa bbbb cccc dddd\nee ff\n");
        let first = layout(&doc, 0, 30.0, &STYLE);
        assert_eq!(texts(&first), vec!["aaaa bbbb", "cccc dddd", "ee ff"]);
        assert_eq!(first.end, Locator::new(0, 26));

        let small = layout(&doc, 0, 10.0, &STYLE);
        assert_eq!(texts(&small), vec!["aaaa bbbb"]);
        assert_eq!(small.end.offset, 10);
        let next = layout(&doc, small.end.offset, 10.0, &STYLE);
        assert_eq!(texts(&next), vec!["cccc dddd"]);
        assert_eq!(next.lines[0].baseline, 8.0);
    }

    #[test]
    fn indents_and_justifies() {
        let doc = Text::new("aa bb cc dd ee\n");
        let style = Style {
            indent: 3.0,
            justify: true,
            ..STYLE
        };
        let page = layout(&doc, 0, 100.0, &style);
        assert_eq!(texts(&page), vec!["aa bb", "cc dd ee"]);
        assert_eq!(page.lines[0].x, 3.0);
        // 7 px available, 5 used, one space: +2 per space.
        assert_eq!(page.lines[0].word_spacing, 2.0);
        assert_eq!(
            page.lines[1].word_spacing, 0.0,
            "last line is not justified"
        );
    }

    #[test]
    fn past_the_end_is_empty() {
        let doc = Text::new("x\n");
        let page = layout(&doc, 2, 100.0, &STYLE);
        assert!(page.is_past_end());
        assert_eq!(page.end, page.start);
    }

    #[test]
    fn paragraph_spacing_between_blocks() {
        let doc = Text::new("a\nb\n");
        let style = Style {
            paragraph_spacing: 5.0,
            ..STYLE
        };
        let page = layout(&doc, 0, 100.0, &style);
        assert_eq!(page.lines[1].baseline - page.lines[0].baseline, 15.0);
    }

    #[test]
    fn styled_runs_are_measured_and_reported() {
        // "*bb*" is bold: 4 characters of 2 units, so "a *bb*" just fits.
        let doc = Text::new("a *bb* c\n");
        let page = layout(&doc, 0, 100.0, &STYLE);
        assert_eq!(texts(&page), vec!["a *bb*", "c"]);
        let faces: Vec<(usize, usize, bool)> = page.lines[0]
            .runs
            .iter()
            .map(|r| (r.start, r.end, r.face.style.bold()))
            .collect();
        assert_eq!(faces, vec![(0, 2, false), (2, 6, true)]);
    }

    #[test]
    fn headings_use_their_face_and_centred_blocks_are_centred() {
        let doc = Text::new("#ab\n>xy\n");
        let style = Style {
            indent: 3.0,
            justify: true,
            ..STYLE
        };
        let page = layout(&doc, 0, 100.0, &style);
        // Heading: 3 characters of 2 units, line box of 20 with ascent 16.
        assert_eq!(page.lines[0].baseline, 16.0);
        assert_eq!(page.lines[0].x, 0.0, "headings are not indented");
        assert_eq!(page.lines[0].runs[0].face.heading, 1);
        // ">xy" is 3 units in a 10-unit line, centred and not indented.
        assert_eq!(page.lines[1].x, 3.5);
        assert_eq!(page.lines[1].baseline, 20.0 + 8.0);
    }

    #[test]
    fn headings_are_not_left_alone_at_the_bottom() {
        let doc = Text::new("aa\n#h\nbb\n");
        // Room for "aa" (10) and the heading (20), but not for "bb".
        let page = layout(&doc, 0, 35.0, &STYLE);
        assert_eq!(texts(&page), vec!["aa"]);
        assert_eq!(page.end, Locator::new(0, 3));
        let next = layout(&doc, page.end.offset, 35.0, &STYLE);
        assert_eq!(texts(&next), vec!["#h", "bb"]);
    }
}

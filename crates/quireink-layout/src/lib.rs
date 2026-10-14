//! Page layout: turns a [`Document`] into pages.
//!
//! Layout is deterministic and starts at any [`Locator`] that begins a line,
//! so a reader only needs to remember where pages start — never the pages
//! themselves. Text is broken greedily per paragraph; because greedy breaking
//! is stable, laying out from the middle of a paragraph yields exactly the
//! same lines as laying out the whole paragraph.
//!
//! The engine knows nothing about fonts or drawing; it measures characters
//! through a callback and returns positioned lines.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use quireink_document::{BlockKind, Document, DocumentError, Locator, Storage};
pub use quireink_text::Hyphenate;
use quireink_text::wrap_lines_hyphenated;

/// Largest extra space per word gap for justification, relative to the
/// line height.
const MAX_JUSTIFY_GAP: f32 = 0.6;

/// Typographic settings for a page, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Distance from the top of a line box to its baseline.
    pub ascent: f32,
    /// Distance between baselines.
    pub line_height: f32,
    /// Extra space between paragraphs.
    pub paragraph_spacing: f32,
    /// Indent of the first line of a paragraph.
    pub indent: f32,
    /// Stretch spaces so lines fill the width (except a paragraph's last).
    pub justify: bool,
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
}

/// One laid-out page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    /// Position of the first character on the page.
    pub start: Locator,
    /// Position where the next page starts; equal to `start` past the end.
    pub end: Locator,
    pub lines: Vec<PageLine>,
}

impl Page {
    /// Whether this page is past the end of the document.
    pub fn is_past_end(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Lays out the page starting at `start` into a `width`×`height` content
/// box. `advance` returns the width of a character; with `hyphenate`, words
/// that do not fit at the end of a line are hyphenated.
#[allow(clippy::too_many_arguments)]
pub fn layout_page(
    document: &dyn Document,
    storage: &dyn Storage,
    start: Locator,
    width: f32,
    height: f32,
    style: &Style,
    advance: &dyn Fn(char) -> f32,
    hyphenate: Option<Hyphenate<'_>>,
) -> Result<Page, DocumentError> {
    let mut page = Page {
        start,
        end: start,
        lines: Vec::new(),
    };
    let mut at = start;
    let mut y = 0.0f32;

    loop {
        let Some(block) = document.block_at(storage, at)? else {
            // End of the section: chapters start on a new page.
            if at.section + 1 < document.sections() && page.lines.is_empty() {
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
        if page.lines.is_empty() && from == 0 && at.offset < block.start {
            page.start = Locator::new(at.section, block.start);
        }
        let at_block_start = from == 0 && !block.continues;
        if !page.lines.is_empty() && at_block_start {
            y += style.paragraph_spacing;
        }
        let indent = if at_block_start && block.kind == BlockKind::Paragraph {
            style.indent
        } else {
            0.0
        };
        let text = &block.text[from..];
        let lines = wrap_lines_hyphenated(
            text,
            |n| if n == 0 { width - indent } else { width },
            advance,
            hyphenate,
        );

        for (n, line) in lines.iter().enumerate() {
            let fits = y + style.line_height <= height;
            if !fits && !page.lines.is_empty() {
                page.end = Locator::new(at.section, block.source_offset(from + line.range.start));
                return Ok(page);
            }
            let x = if n == 0 { indent } else { 0.0 };
            let available = if n == 0 { width - indent } else { width };
            let word_spacing = if style.justify && !line.last && line.spaces > 0 {
                let extra = (available - line.width) / line.spaces as f32;
                // Huge gaps look worse than a ragged line: set those lines
                // flush left (e.g. a few words before a very long word).
                if extra > style.line_height * MAX_JUSTIFY_GAP {
                    0.0
                } else {
                    extra.max(0.0)
                }
            } else {
                0.0
            };
            let mut line_text = String::from(&text[line.range.clone()]);
            if line.hyphen {
                line_text.push('-');
            }
            page.lines.push(PageLine {
                x,
                baseline: y + style.ascent,
                text: line_text,
                word_spacing,
            });
            y += style.line_height;
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
    use quireink_document::{Block, Metadata, OffsetMap};

    /// Paragraphs separated by single newlines; offsets are bytes.
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
            Ok(Some(Block {
                kind: BlockKind::Paragraph,
                continues: false,
                start: start as u32,
                end: end as u32,
                text: String::from(self.text[start..end].trim_end_matches('\n')),
                map: OffsetMap::IDENTITY,
            }))
        }
    }

    const STYLE: Style = Style {
        ascent: 8.0,
        line_height: 10.0,
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
            &|_| 1.0,
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
}

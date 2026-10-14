//! A small CSS subset for EPUB stylesheets.
//!
//! Selectors: `tag`, `.class`, `tag.class` and lists of them; anything
//! more complex (descendants, pseudo-classes, attributes) is skipped, as
//! are `@` rules. Properties: `text-align`, `font-weight`, `font-style` and
//! `display: none`. That covers how books mark centred title pages, bold
//! and italic classes and hidden elements.
//!
//! The parser is fed text in chunks and keeps only the rule being read.

use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::document::Align;

/// What a rule or a `style` attribute sets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Props {
    pub align: Option<Align>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub hidden: Option<bool>,
}

impl Props {
    pub fn is_empty(&self) -> bool {
        *self == Props::default()
    }

    /// `other` overrides what it sets.
    pub fn apply(&mut self, other: &Props) {
        self.align = other.align.or(self.align);
        self.bold = other.bold.or(self.bold);
        self.italic = other.italic.or(self.italic);
        self.hidden = other.hidden.or(self.hidden);
    }

    /// Reads declarations such as `text-align: center; font-weight: bold`.
    pub fn parse(declarations: &str) -> Props {
        let mut props = Props::default();
        for declaration in declarations.split(';') {
            let Some((property, value)) = declaration.split_once(':') else {
                continue;
            };
            let value = value
                .trim()
                .trim_end_matches("!important")
                .trim()
                .to_ascii_lowercase();
            match property.trim().to_ascii_lowercase().as_str() {
                "text-align" => props.align = parse_align(&value).or(props.align),
                "font-weight" => {
                    props.bold = match value.as_str() {
                        "bold" | "bolder" | "600" | "700" | "800" | "900" => Some(true),
                        "normal" | "lighter" | "100" | "200" | "300" | "400" | "500" => Some(false),
                        _ => props.bold,
                    }
                }
                "font-style" => {
                    props.italic = match value.as_str() {
                        "italic" | "oblique" => Some(true),
                        "normal" => Some(false),
                        _ => props.italic,
                    }
                }
                "display" => {
                    props.hidden = Some(value == "none");
                }
                _ => {}
            }
        }
        props
    }

    /// Compact text form for the cache, e.g. `a2 b1 i0 h1`.
    pub fn encode(&self, out: &mut String) {
        if let Some(align) = self.align {
            out.push_str(&alloc::format!(" a{}", align.bits()));
        }
        for (key, value) in [('b', self.bold), ('i', self.italic), ('h', self.hidden)] {
            if let Some(value) = value {
                out.push(' ');
                out.push(key);
                out.push(if value { '1' } else { '0' });
            }
        }
    }

    pub fn decode(text: &str) -> Props {
        let mut props = Props::default();
        for item in text.split_whitespace() {
            let mut chars = item.chars();
            let (Some(key), Some(value)) = (chars.next(), chars.next()) else {
                continue;
            };
            let flag = value == '1';
            match key {
                'a' => props.align = value.to_digit(10).map(|d| Align::from_bits(d as u8)),
                'b' => props.bold = Some(flag),
                'i' => props.italic = Some(flag),
                'h' => props.hidden = Some(flag),
                _ => {}
            }
        }
        props
    }
}

pub(crate) fn parse_align(value: &str) -> Option<Align> {
    match value.trim().to_ascii_lowercase().as_str() {
        "center" => Some(Align::Center),
        "right" | "end" => Some(Align::End),
        "left" | "start" => Some(Align::Start),
        "justify" => Some(Align::Justify),
        _ => None,
    }
}

/// A simple selector: element name and/or one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selector {
    pub tag: Option<String>,
    pub class: Option<String>,
}

impl Selector {
    fn parse(text: &str) -> Option<Selector> {
        let text = text.trim();
        if text.is_empty()
            || text
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '>' | '+' | '~' | ':' | '[' | '#' | '*'))
        {
            return None;
        }
        let (tag, class) = match text.split_once('.') {
            Some((tag, class)) if !class.contains('.') => (tag, Some(class)),
            Some(_) => return None,
            None => (text, None),
        };
        Some(Selector {
            tag: (!tag.is_empty()).then(|| tag.to_ascii_lowercase()),
            class: class.map(String::from),
        })
    }

    pub fn matches(&self, tag: &str, classes: &str) -> bool {
        self.tag.as_deref().is_none_or(|t| t == tag)
            && self
                .class
                .as_deref()
                .is_none_or(|c| classes.split_ascii_whitespace().any(|k| k == c))
    }

    /// Class selectors outrank element selectors.
    pub fn specificity(&self) -> u8 {
        u8::from(self.class.is_some()) * 2 + u8::from(self.tag.is_some())
    }
}

/// Stylesheet rules in source order.
#[derive(Debug, Default)]
pub(crate) struct Rules {
    pub rules: Vec<(Selector, Props)>,
}

/// Rules read beyond this are dropped (memory bound on small devices).
const MAX_RULES: usize = 512;

impl Rules {
    /// What the rules set for an element, by specificity, then order.
    pub fn props(&self, tag: &str, classes: &str) -> Props {
        let mut props = Props::default();
        for level in 0..=3 {
            for (selector, rule) in &self.rules {
                if selector.specificity() == level && selector.matches(tag, classes) {
                    props.apply(rule);
                }
            }
        }
        props
    }

    fn add(&mut self, selectors: &str, declarations: &str) {
        let props = Props::parse(declarations);
        if props.is_empty() {
            return;
        }
        for selector in selectors.split(',').filter_map(Selector::parse) {
            if self.rules.len() < MAX_RULES {
                self.rules.push((selector, props));
            }
        }
    }

    /// Cache form: one `selector<TAB>props` line per rule.
    pub fn encode(&self) -> String {
        let mut out = String::new();
        for (selector, props) in &self.rules {
            if let Some(tag) = &selector.tag {
                out.push_str(tag);
            }
            if let Some(class) = &selector.class {
                out.push('.');
                out.push_str(class);
            }
            out.push('\t');
            props.encode(&mut out);
            out.push('\n');
        }
        out
    }

    pub fn decode(text: &str) -> Rules {
        let rules = text
            .lines()
            .filter_map(|line| {
                let (selector, props) = line.split_once('\t')?;
                Some((Selector::parse(selector)?, Props::decode(props)))
            })
            .collect();
        Rules { rules }
    }
}

/// Reads a stylesheet in chunks.
#[derive(Debug, Default)]
pub(crate) struct Parser {
    /// Text of the current selector list or declaration block.
    buffer: String,
    selectors: String,
    in_block: bool,
    in_comment: bool,
    /// Depth inside an `@` rule's braces, which are skipped.
    skip_depth: u32,
    previous: char,
}

/// Longest selector list or declaration block that is kept.
const MAX_PART: usize = 2048;

impl Parser {
    pub fn feed(&mut self, text: &str, rules: &mut Rules) {
        for c in text.chars() {
            let previous = core::mem::replace(&mut self.previous, c);
            if self.in_comment {
                if previous == '*' && c == '/' {
                    self.in_comment = false;
                    self.previous = ' ';
                }
                continue;
            }
            if previous == '/' && c == '*' {
                self.buffer.pop();
                self.in_comment = true;
                continue;
            }
            if self.skip_depth > 0 {
                match c {
                    '{' => self.skip_depth += 1,
                    '}' => self.skip_depth -= 1,
                    _ => {}
                }
                continue;
            }
            match c {
                '{' if !self.in_block => {
                    if self.buffer.trim_start().starts_with('@') {
                        self.skip_depth = 1;
                        self.buffer.clear();
                    } else {
                        self.selectors = core::mem::take(&mut self.buffer);
                        self.in_block = true;
                    }
                }
                '}' if self.in_block => {
                    rules.add(&self.selectors, &self.buffer);
                    self.buffer.clear();
                    self.selectors.clear();
                    self.in_block = false;
                }
                ';' if !self.in_block && self.buffer.trim_start().starts_with('@') => {
                    // `@import …;`, `@charset …;`
                    self.buffer.clear();
                }
                c if self.buffer.len() < MAX_PART => self.buffer.push(c),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(css: &str) -> Rules {
        let mut rules = Rules::default();
        let mut parser = Parser::default();
        // Fed in small pieces, as from a stream.
        for piece in css.as_bytes().chunks(7) {
            parser.feed(core::str::from_utf8(piece).unwrap(), &mut rules);
        }
        rules
    }

    #[test]
    fn reads_simple_rules() {
        let r = rules(
            "@charset \"utf-8\"; /* title page */ h1, h2 { text-align: center } \
             .center{text-align:center;} p.bold { font-weight: bold } \
             @media print { p { display: none } } div p { font-style: italic } \
             .hide { display: none } em.normal { font-style: normal !important }",
        );
        assert_eq!(r.rules.len(), 6);
        assert_eq!(r.props("h2", "").align, Some(Align::Center));
        assert_eq!(r.props("p", "x center").align, Some(Align::Center));
        assert_eq!(r.props("p", "bold").bold, Some(true));
        assert_eq!(r.props("div", "bold").bold, None);
        assert_eq!(r.props("span", "hide").hidden, Some(true));
        assert_eq!(r.props("em", "normal").italic, Some(false));
        // Descendant selectors and @media blocks are skipped.
        assert_eq!(r.props("p", "").italic, None);
        assert_eq!(r.props("p", "").hidden, None);
    }

    #[test]
    fn classes_outrank_elements_and_survive_the_cache() {
        let r = rules(".left { text-align: left } p { text-align: center }");
        assert_eq!(r.props("p", "left").align, Some(Align::Start));
        assert_eq!(r.props("p", "").align, Some(Align::Center));
        let copy = Rules::decode(&r.encode());
        assert_eq!(copy.rules, r.rules);
    }
}

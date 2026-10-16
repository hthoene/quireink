//! A small CSS subset for EPUB stylesheets.
//!
//! Selectors: `tag`, `.class`, `tag.class` and lists of them; anything
//! more complex (descendants, pseudo-classes, attributes) is skipped, as
//! are `@` rules except `@font-face`. Properties: `text-align`,
//! `font-weight`, `font-style`, `font-family`, `display: none`,
//! `text-indent`, `font-size` and the vertical `margin` and `padding`.
//! That covers how
//! books mark centred title pages, bold and italic classes, hidden
//! elements, their embedded fonts, paragraphs without indent and the
//! space around blocks.
//!
//! The parser is fed text in chunks and keeps only the rule being read.

use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::document::{Align, MAX_FAMILIES};

use crate::package::resolve;

/// What a rule or a `style` attribute sets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Props {
    pub align: Option<Align>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// `font-variant: small-caps`.
    pub small_caps: Option<bool>,
    pub hidden: Option<bool>,
    /// A font family: its number in [`Rules::families`] plus one; 0 for
    /// the reader's font (a generic family such as `serif`).
    pub family: Option<u8>,
    /// `text-indent`, in hundredths of an em.
    pub indent: Option<i16>,
    /// `margin-top`, `margin-bottom`, `padding-top` and `padding-bottom`,
    /// in hundredths of an em.
    pub margin_top: Option<u16>,
    pub margin_bottom: Option<u16>,
    pub padding_top: Option<u16>,
    pub padding_bottom: Option<u16>,
    /// `font-size`.
    pub size: Option<FontSize>,
}

/// A `font-size`, in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontSize {
    /// Of the parent's size (`em`, `%`, `smaller`, …).
    Relative(u16),
    /// Of body text (`pt`, `px`, `rem`, `large`, …).
    Absolute(u16),
}

impl FontSize {
    /// The size in percent of body text, inside an element of `parent`
    /// percent; limited to what stays readable and fits a page.
    pub fn of(self, parent: u16) -> u16 {
        let size = match self {
            FontSize::Relative(percent) => u32::from(parent) * u32::from(percent) / 100,
            FontSize::Absolute(percent) => u32::from(percent),
        };
        size.clamp(MIN_SIZE, MAX_SIZE) as u16
    }
}

/// Text sizes taken from a book, in percent of body text.
const MIN_SIZE: u32 = 50;
const MAX_SIZE: u32 = 300;

impl Props {
    pub fn is_empty(&self) -> bool {
        *self == Props::default()
    }

    /// `other` overrides what it sets.
    pub fn apply(&mut self, other: &Props) {
        self.align = other.align.or(self.align);
        self.bold = other.bold.or(self.bold);
        self.italic = other.italic.or(self.italic);
        self.small_caps = other.small_caps.or(self.small_caps);
        self.hidden = other.hidden.or(self.hidden);
        self.family = other.family.or(self.family);
        self.indent = other.indent.or(self.indent);
        self.margin_top = other.margin_top.or(self.margin_top);
        self.margin_bottom = other.margin_bottom.or(self.margin_bottom);
        self.padding_top = other.padding_top.or(self.padding_top);
        self.padding_bottom = other.padding_bottom.or(self.padding_bottom);
        self.size = other.size.or(self.size);
    }

    /// Reads declarations such as `text-align: center; font-weight: bold`;
    /// font families are looked up in (and added to) `families`.
    pub fn parse_with(declarations: &str, families: &mut Vec<String>) -> Props {
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
                "font-variant" | "font-variant-caps" => {
                    props.small_caps =
                        if value.contains("small-caps") || value.contains("petite-caps") {
                            Some(true)
                        } else if value == "normal" || value == "none" {
                            Some(false)
                        } else {
                            props.small_caps
                        }
                }
                "display" => {
                    props.hidden = Some(value == "none");
                }
                "font-family" => props.family = Some(family_number(&value, families)),
                "font-size" => props.size = font_size(&value).or(props.size),
                "text-indent" => {
                    // Negative indents hang out of a margin, which is not
                    // supported: no indent instead.
                    let indent = length(&value).map(|l| l.clamp(0, MAX_INDENT) as i16);
                    props.indent = indent.or(props.indent);
                }
                "margin-top" => props.margin_top = space(&value).or(props.margin_top),
                "margin-bottom" => props.margin_bottom = space(&value).or(props.margin_bottom),
                "padding-top" => props.padding_top = space(&value).or(props.padding_top),
                "padding-bottom" => {
                    props.padding_bottom = space(&value).or(props.padding_bottom);
                }
                "margin" | "padding" => {
                    // One to four values: the first is the top, the third
                    // (or else the first) the bottom.
                    let values: Vec<&str> = value.split_whitespace().collect();
                    let top = values.first().and_then(|v| space(v));
                    let bottom = values.get(2).or(values.first()).and_then(|v| space(v));
                    if property.trim().eq_ignore_ascii_case("margin") {
                        props.margin_top = top.or(props.margin_top);
                        props.margin_bottom = bottom.or(props.margin_bottom);
                    } else {
                        props.padding_top = top.or(props.padding_top);
                        props.padding_bottom = bottom.or(props.padding_bottom);
                    }
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
        if let Some(family) = self.family {
            out.push_str(&alloc::format!(" f{family}"));
        }
        if let Some(indent) = self.indent {
            out.push_str(&alloc::format!(" t{indent}"));
        }
        match self.size {
            Some(FontSize::Relative(percent)) => out.push_str(&alloc::format!(" r{percent}")),
            Some(FontSize::Absolute(percent)) => out.push_str(&alloc::format!(" z{percent}")),
            None => {}
        }
        for (key, value) in [
            ('m', self.margin_top),
            ('n', self.margin_bottom),
            ('p', self.padding_top),
            ('q', self.padding_bottom),
        ] {
            if let Some(value) = value {
                out.push_str(&alloc::format!(" {key}{value}"));
            }
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
                'f' => props.family = item[1..].parse().ok(),
                't' => props.indent = item[1..].parse().ok(),
                'm' => props.margin_top = item[1..].parse().ok(),
                'n' => props.margin_bottom = item[1..].parse().ok(),
                'p' => props.padding_top = item[1..].parse().ok(),
                'q' => props.padding_bottom = item[1..].parse().ok(),
                'r' => props.size = item[1..].parse().ok().map(FontSize::Relative),
                'z' => props.size = item[1..].parse().ok().map(FontSize::Absolute),
                _ => {}
            }
        }
        props
    }
}

/// The largest indent and space taken from a book, in hundredths of an
/// em, so a stray value cannot push text off the page.
const MAX_INDENT: i32 = 500;
const MAX_SPACE: i32 = 800;

/// A CSS length in hundredths of an em: `em` and `rem` as they are, `pt`
/// and `px` at the usual 12 pt and 16 px to the em, `0` without a unit.
/// `None` for other units (e.g. `%`) and keywords such as `auto`.
pub(crate) fn length(value: &str) -> Option<i32> {
    let value = value.trim();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f32 = number.parse().ok()?;
    let em = match unit.trim() {
        "em" | "rem" => number,
        "ex" => number / 2.0,
        "pt" => number / 12.0,
        "px" => number / 16.0,
        "" if number == 0.0 => 0.0,
        _ => return None,
    };
    // Rounded (no_std has no `f32::round`).
    let half = if em < 0.0 { -0.5 } else { 0.5 };
    Some((em * 100.0 + half) as i32)
}

/// A `font-size` value; `None` for what is not understood.
fn font_size(value: &str) -> Option<FontSize> {
    let keyword = match value.trim() {
        "xx-small" => Some(FontSize::Absolute(60)),
        "x-small" => Some(FontSize::Absolute(75)),
        "small" => Some(FontSize::Absolute(89)),
        "medium" => Some(FontSize::Absolute(100)),
        "large" => Some(FontSize::Absolute(120)),
        "x-large" => Some(FontSize::Absolute(150)),
        "xx-large" => Some(FontSize::Absolute(200)),
        "smaller" => Some(FontSize::Relative(83)),
        "larger" => Some(FontSize::Relative(120)),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        let percent: f32 = percent.trim().parse().ok()?;
        return (percent > 0.0).then_some(FontSize::Relative((percent + 0.5) as u16));
    }
    let percent = u16::try_from(length(value)?).ok().filter(|&p| p > 0)?;
    // Lengths in `em` are relative to the parent; `rem`, `pt` and `px` to
    // body text.
    Some(if value.ends_with("em") && !value.ends_with("rem") {
        FontSize::Relative(percent)
    } else {
        FontSize::Absolute(percent)
    })
}

/// A vertical margin or padding: not negative, limited.
fn space(value: &str) -> Option<u16> {
    length(value).map(|l| l.clamp(0, MAX_SPACE) as u16)
}

/// The sum of two spaces, `None` if neither is set.
pub(crate) fn sum(a: Option<u16>, b: Option<u16>) -> Option<u16> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    }
}

/// The number of the first family in a `font-family` list: its place in
/// `families` plus one, added if new; 0 for a generic family.
fn family_number(value: &str, families: &mut Vec<String>) -> u8 {
    let Some(name) = value.split(',').map(family_name).find(|n| !n.is_empty()) else {
        return 0;
    };
    const GENERIC: [&str; 9] = [
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "inherit",
        "initial",
        "unset",
    ];
    if GENERIC.contains(&name.as_str()) {
        return 0;
    }
    match families.iter().position(|f| *f == name) {
        Some(at) => at as u8 + 1,
        None if families.len() < usize::from(MAX_FAMILIES) => {
            families.push(name);
            families.len() as u8
        }
        None => 0,
    }
}

/// A family name as compared: without quotes, in lower case.
fn family_name(text: &str) -> String {
    text.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_ascii_lowercase()
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

/// Stylesheet rules in source order, with the font families they name
/// and the fonts the stylesheets embed.
#[derive(Debug, Default)]
pub(crate) struct Rules {
    pub rules: Vec<(Selector, Props)>,
    /// Font family names in the order first named.
    pub families: Vec<String>,
    pub faces: Vec<FontFace>,
    /// One's own rules (see [`Rules::add_own`]): after all others.
    own: Vec<(Selector, Props)>,
}

/// A font embedded with `@font-face`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FontFace {
    /// As in [`Props::family`].
    pub family: u8,
    pub bold: bool,
    pub italic: bool,
    /// The font file in the archive.
    pub path: String,
}

/// Rules read beyond this are dropped (memory bound on small devices).
const MAX_RULES: usize = 512;

impl Rules {
    /// What the rules set for an element, by specificity, then order.
    pub fn props(&self, tag: &str, classes: &str) -> Props {
        let mut props = Props::default();
        // The book's rules, then one's own over them.
        for rules in [&self.rules, &self.own] {
            for level in 0..=3 {
                for (selector, rule) in rules {
                    if selector.specificity() == level && selector.matches(tag, classes) {
                        props.apply(rule);
                    }
                }
            }
        }
        props
    }

    /// Adds one's own stylesheet `css`: its rules apply after all of the
    /// book's, also those it adds later (`<style>` in a chapter).
    pub fn add_own(&mut self, css: &str) {
        let start = self.rules.len();
        Parser::new("").feed(css, self);
        let own = self.rules.split_off(start);
        self.own.extend(own);
    }

    /// Parses a `style` attribute.
    pub fn parse_style(&mut self, declarations: &str) -> Props {
        Props::parse_with(declarations, &mut self.families)
    }

    /// Adds an `@font-face` rule of a stylesheet in folder `base`. Only
    /// OpenType and TrueType files are used (not WOFF).
    fn add_face(&mut self, declarations: &str, base: &str) {
        let mut name = String::new();
        let mut src = None;
        let (mut bold, mut italic) = (false, false);
        for declaration in declarations.split(';') {
            let Some((property, value)) = declaration.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match property.trim().to_ascii_lowercase().as_str() {
                "font-family" => name = family_name(value),
                "font-weight" => {
                    let v = value.to_ascii_lowercase();
                    bold = v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600);
                }
                "font-style" => {
                    let v = value.to_ascii_lowercase();
                    italic = v == "italic" || v == "oblique";
                }
                "src" => src = font_url(value),
                _ => {}
            }
        }
        let Some(url) = src else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let family = family_number(&name, &mut self.families);
        if family == 0 {
            return;
        }
        let (path, _) = resolve(base, &url);
        self.faces.push(FontFace {
            family,
            bold,
            italic,
            path,
        });
    }

    fn add(&mut self, selectors: &str, declarations: &str) {
        let props = Props::parse_with(declarations, &mut self.families);
        if props.is_empty() {
            return;
        }
        for selector in selectors.split(',').filter_map(Selector::parse) {
            if self.rules.len() < MAX_RULES {
                self.rules.push((selector, props));
            }
        }
    }

    /// Cache form: one `@family<TAB>name` line per family, then one
    /// `selector<TAB>props` line per rule (embedded fonts are kept apart,
    /// see `package`).
    pub fn encode(&self) -> String {
        let mut out = String::new();
        for family in &self.families {
            out.push_str("@family\t");
            out.push_str(family);
            out.push('\n');
        }
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
        let mut families = Vec::new();
        let rules = text
            .lines()
            .filter_map(|line| {
                let (selector, props) = line.split_once('\t')?;
                if selector == "@family" {
                    families.push(String::from(props));
                    return None;
                }
                Some((Selector::parse(selector)?, Props::decode(props)))
            })
            .collect();
        Rules {
            rules,
            families,
            faces: Vec::new(),
            own: Vec::new(),
        }
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
    /// Inside an `@font-face` block.
    in_face: bool,
    previous: char,
    /// Folder of the stylesheet, for font URLs.
    base: String,
}

/// Longest selector list or declaration block that is kept.
const MAX_PART: usize = 2048;

impl Parser {
    /// A parser for a stylesheet in folder `base` (ending in `/`).
    pub fn new(base: &str) -> Parser {
        Parser {
            base: String::from(base),
            ..Parser::default()
        }
    }

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
                    let head = self.buffer.trim_start().to_ascii_lowercase();
                    if head.starts_with("@font-face") {
                        self.in_face = true;
                        self.in_block = true;
                        self.buffer.clear();
                    } else if head.starts_with('@') {
                        self.skip_depth = 1;
                        self.buffer.clear();
                    } else {
                        self.selectors = core::mem::take(&mut self.buffer);
                        self.in_block = true;
                    }
                }
                '}' if self.in_block && self.in_face => {
                    rules.add_face(&self.buffer, &self.base);
                    self.buffer.clear();
                    self.in_face = false;
                    self.in_block = false;
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

/// The first OpenType or TrueType file of a `src` list such as
/// `url("a.woff2") format("woff2"), url(a.ttf) format("truetype")`.
fn font_url(src: &str) -> Option<String> {
    src.split("url(").skip(1).find_map(|part| {
        let (url, rest) = part.split_once(')')?;
        let url = url.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        let format = rest
            .split(',')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let lower = url.to_ascii_lowercase();
        let by_name = [".ttf", ".otf", ".ttc"].iter().any(|e| lower.ends_with(e));
        let by_format = format.contains("truetype") || format.contains("opentype");
        let other =
            format.contains("woff") || format.contains("svg") || format.contains("embedded");
        ((by_name && !other) || by_format).then(|| String::from(url))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(css: &str) -> Rules {
        let mut rules = Rules::default();
        let mut parser = Parser::new("OEBPS/css/");
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
    fn reads_indents_and_spaces() {
        assert_eq!(length("1.2em"), Some(120));
        assert_eq!(length("0"), Some(0));
        assert_eq!(length("12pt"), Some(100));
        assert_eq!(length("8px"), Some(50));
        assert_eq!(length("5%"), None);
        assert_eq!(length("auto"), None);
        let r = rules(
            ".noindent { text-indent:0em; margin-top:1.2em }              .head { padding-top: 6em; padding-bottom: 1.0em }              .hang { text-indent: -1.5em; margin: 0.5em 0 1em 2em } p { margin: 0 }",
        );
        let p = r.props("p", "noindent");
        assert_eq!(
            (p.indent, p.margin_top, p.margin_bottom),
            (Some(0), Some(120), Some(0))
        );
        let h = r.props("p", "head");
        assert_eq!((h.padding_top, h.padding_bottom), (Some(600), Some(100)));
        // Hanging indents need a margin we do not support: none instead.
        let hang = r.props("div", "hang");
        assert_eq!(hang.indent, Some(0));
        assert_eq!((hang.margin_top, hang.margin_bottom), (Some(50), Some(100)));
        let copy = Rules::decode(&r.encode());
        assert_eq!(copy.rules, r.rules);
    }

    #[test]
    fn reads_font_sizes() {
        let r = rules(
            ".big { font-size: 1.3em } .half { font-size: 50% } .pt { font-size: 9pt } \
             .k { font-size: x-large } .s { font-size: smaller } .bad { font-size: inherit }",
        );
        let size = |class| r.props("p", class).size;
        assert_eq!(size("big"), Some(FontSize::Relative(130)));
        assert_eq!(size("half"), Some(FontSize::Relative(50)));
        assert_eq!(size("pt"), Some(FontSize::Absolute(75)));
        assert_eq!(size("k"), Some(FontSize::Absolute(150)));
        assert_eq!(size("s"), Some(FontSize::Relative(83)));
        assert_eq!(size("bad"), None);
        // Relative sizes compound, and all stay readable.
        assert_eq!(FontSize::Relative(130).of(120), 156);
        assert_eq!(FontSize::Absolute(75).of(200), 75);
        assert_eq!(FontSize::Relative(50).of(50), 50);
        let copy = Rules::decode(&r.encode());
        assert_eq!(copy.rules, r.rules);
    }

    #[test]
    fn classes_outrank_elements_and_survive_the_cache() {
        let r = rules(".left { text-align: left } p { text-align: center }");
        assert_eq!(r.props("p", "left").align, Some(Align::Start));
        assert_eq!(r.props("p", "").align, Some(Align::Center));
        let copy = Rules::decode(&r.encode());
        assert_eq!(copy.rules, r.rules);
    }

    #[test]
    fn reads_embedded_fonts_and_families() {
        let r = rules(
            "@font-face { font-family: 'My Serif'; src: url(../fonts/a.woff2) format('woff2'), \
             url(\"../fonts/MySerif.ttf\") format('truetype') } \
             @font-face { font-family: \"My Serif\"; font-weight: bold; font-style: italic; \
             src: url(../fonts/MySerif-BI.otf) } \
             @font-face { font-family: Web; src: url(web.woff) } \
             p { font-family: \"My Serif\", serif } h1 { font-family: sans-serif } \
             .other { font-family: Unknown, serif }",
        );
        // "Web" only has a WOFF file, so it is not embedded.
        assert_eq!(r.families, ["my serif", "unknown"]);
        assert_eq!(
            r.faces,
            [
                FontFace {
                    family: 1,
                    bold: false,
                    italic: false,
                    path: String::from("OEBPS/fonts/MySerif.ttf"),
                },
                FontFace {
                    family: 1,
                    bold: true,
                    italic: true,
                    path: String::from("OEBPS/fonts/MySerif-BI.otf"),
                },
            ]
        );
        assert_eq!(r.props("p", "").family, Some(1));
        assert_eq!(r.props("h1", "").family, Some(0));
        assert_eq!(r.props("p", "other").family, Some(2));
        let copy = Rules::decode(&r.encode());
        assert_eq!(copy.families, r.families);
        assert_eq!(copy.props("p", "").family, Some(1));
    }
}

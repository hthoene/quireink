//! A small CSS subset for EPUB stylesheets.
//!
//! Selectors: `tag`, `.class`, `tag.class` and lists of them; anything
//! more complex (descendants, pseudo-classes, attributes) is skipped, as
//! are `@` rules except `@font-face`. Properties: `text-align`,
//! `font-weight`, `font-style`, `font-family` and `display: none`. That
//! covers how books mark centred title pages, bold and italic classes,
//! hidden elements and their embedded fonts.
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
    pub hidden: Option<bool>,
    /// A font family: its number in [`Rules::families`] plus one; 0 for
    /// the reader's font (a generic family such as `serif`).
    pub family: Option<u8>,
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
        self.family = other.family.or(self.family);
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
                "display" => {
                    props.hidden = Some(value == "none");
                }
                "font-family" => props.family = Some(family_number(&value, families)),
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
                _ => {}
            }
        }
        props
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
        for level in 0..=3 {
            for (selector, rule) in &self.rules {
                if selector.specificity() == level && selector.matches(tag, classes) {
                    props.apply(rule);
                }
            }
        }
        props
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

//! A tiny, human-editable configuration format.
//!
//! QuireInk stores settings, key maps, themes and translations in the same
//! simple INI dialect, so users can edit everything with a text editor on
//! the device's storage. The parser is zero-copy, `no_std` and only allocates
//! when a quoted value contains escape sequences.
//!
//! # Format
//!
//! ```ini
//! # Comments start with '#' or ';' at the beginning of a line.
//! top-level-key = values before the first section have an empty section name
//!
//! [display]
//! full-refresh-interval = 6
//! night-mode = off
//!
//! [messages]
//! greeting = Hello, {name}!
//! padded = "  quoted values keep spaces, and support \"escapes\"\n"
//! ```
//!
//! - Keys and section names consist of ASCII letters, digits, `_`, `-` and `.`.
//! - Unquoted values are taken literally (trimmed). There are no inline
//!   comments, so values may contain `#` and `;`.
//! - Quoted values support the escapes `\\`, `\"`, `\n`, `\t` and `\u{…}`.
//! - Later entries override earlier ones; that is up to the consumer.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::string::String;
use core::fmt;

/// Parses `source` into an iterator of entries.
pub fn parse(source: &str) -> Parser<'_> {
    Parser {
        lines: source.lines(),
        line: 0,
        section: "",
    }
}

/// A `key = value` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The enclosing section, `""` before the first section header.
    pub section: &'a str,
    pub key: &'a str,
    pub value: Value<'a>,
    /// 1-based line number, for error messages.
    pub line: u32,
}

/// A value as written in the source. Its escapes have already been validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Value<'a> {
    raw: &'a str,
    quoted: bool,
}

impl<'a> Value<'a> {
    /// The value as text, with escape sequences resolved.
    pub fn as_str(&self) -> Cow<'a, str> {
        if !self.quoted || !self.raw.contains('\\') {
            return Cow::Borrowed(self.raw);
        }
        let mut out = String::with_capacity(self.raw.len());
        let mut chars = self.raw.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            // Escapes were validated by the parser.
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let rest = chars.as_str();
                    if let Some((c, len)) = parse_unicode_escape(rest) {
                        out.push(c);
                        chars = rest[len..].chars();
                    }
                }
                Some(other) => out.push(other),
                None => {}
            }
        }
        Cow::Owned(out)
    }

    /// Interprets `true/false`, `yes/no`, `on/off` and `1/0`.
    pub fn as_bool(&self) -> Option<bool> {
        match self.raw {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.raw.parse().ok()
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.raw.parse().ok()
    }

    /// The value exactly as written, without surrounding quotes.
    pub fn raw(&self) -> &'a str {
        self.raw
    }
}

/// A syntax error with its 1-based line number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    pub line: u32,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A line that is neither a comment, a section header nor `key = value`.
    InvalidLine,
    /// A section header without closing `]` or with invalid characters.
    InvalidSection,
    /// A key with invalid characters.
    InvalidKey,
    /// A quoted value without closing quote.
    UnterminatedString,
    /// An unknown or malformed escape sequence.
    InvalidEscape,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            ErrorKind::InvalidLine => "expected `key = value`, `[section]` or a comment",
            ErrorKind::InvalidSection => "invalid section header",
            ErrorKind::InvalidKey => "invalid key",
            ErrorKind::UnterminatedString => "missing closing quote",
            ErrorKind::InvalidEscape => "invalid escape sequence",
        };
        write!(f, "line {}: {}", self.line, message)
    }
}

impl core::error::Error for ParseError {}

/// Iterator over the entries of a configuration source.
#[derive(Debug, Clone)]
pub struct Parser<'a> {
    lines: core::str::Lines<'a>,
    line: u32,
    section: &'a str,
}

impl<'a> Iterator for Parser<'a> {
    type Item = Result<Entry<'a>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let raw_line = self.lines.next()?;
            self.line += 1;
            let line = raw_line.trim();
            let error = |kind| {
                Some(Err(ParseError {
                    line: self.line,
                    kind,
                }))
            };

            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if let Some(header) = line.strip_prefix('[') {
                let Some(name) = header.strip_suffix(']') else {
                    return error(ErrorKind::InvalidSection);
                };
                let name = name.trim();
                if !is_identifier(name) {
                    return error(ErrorKind::InvalidSection);
                }
                self.section = name;
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return error(ErrorKind::InvalidLine);
            };
            let key = key.trim();
            if !is_identifier(key) {
                return error(ErrorKind::InvalidKey);
            }
            let value = match parse_value(value.trim()) {
                Ok(value) => value,
                Err(kind) => return error(kind),
            };
            return Some(Ok(Entry {
                section: self.section,
                key,
                value,
                line: self.line,
            }));
        }
    }
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn parse_value(text: &str) -> Result<Value<'_>, ErrorKind> {
    let Some(inner) = text.strip_prefix('"') else {
        return Ok(Value {
            raw: text,
            quoted: false,
        });
    };

    // Find the closing quote, skipping escaped characters.
    let mut chars = inner.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => {
                if !inner[index + 1..].trim().is_empty() {
                    return Err(ErrorKind::InvalidLine);
                }
                return Ok(Value {
                    raw: &inner[..index],
                    quoted: true,
                });
            }
            '\\' => match chars.next() {
                Some((_, '\\' | '"' | 'n' | 't')) => {}
                Some((u_index, 'u')) => {
                    let len = parse_unicode_escape(&inner[u_index + 1..])
                        .ok_or(ErrorKind::InvalidEscape)?
                        .1;
                    for _ in 0..len {
                        chars.next();
                    }
                }
                Some(_) => return Err(ErrorKind::InvalidEscape),
                None => return Err(ErrorKind::UnterminatedString),
            },
            _ => {}
        }
    }
    Err(ErrorKind::UnterminatedString)
}

/// Parses `{XXXX}` (1–6 hex digits) and returns the char and consumed bytes.
fn parse_unicode_escape(s: &str) -> Option<(char, usize)> {
    let body = s.strip_prefix('{')?;
    let end = body.find('}')?;
    let hex = &body[..end];
    if hex.is_empty() || hex.len() > 6 {
        return None;
    }
    let code = u32::from_str_radix(hex, 16).ok()?;
    Some((char::from_u32(code)?, end + 2))
}

/// Returns `source` with `key` in `section` set to `value`.
///
/// Edits as little as possible, so hand-written files keep their comments,
/// order and formatting: an existing entry's line is replaced, a new entry
/// goes after the section's last entry, and a missing section is appended.
/// Values that need it are quoted and escaped.
pub fn set(source: &str, section: &str, key: &str, value: &str) -> String {
    let line = alloc::format!("{key} = {}", quote(value));
    let lines: alloc::vec::Vec<&str> = source.lines().collect();
    let mut current = "";
    let mut section_found = false;
    let mut last_entry_of_section: Option<usize> = None;
    let mut header_of_section: Option<usize> = None;
    for (i, raw) in lines.iter().enumerate() {
        let text = raw.trim();
        if let Some(name) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            current = name.trim();
            if current == section {
                section_found = true;
                header_of_section = Some(i);
            }
            continue;
        }
        if current != section || text.is_empty() || text.starts_with(['#', ';']) {
            continue;
        }
        if let Some((k, _)) = text.split_once('=') {
            if k.trim() == key {
                let mut out = alloc::vec::Vec::from(&lines[..]);
                out[i] = &line;
                return join(&out);
            }
            last_entry_of_section = Some(i);
        }
    }
    if section_found {
        let at = last_entry_of_section.or(header_of_section).unwrap_or(0) + 1;
        let mut out = alloc::vec::Vec::from(&lines[..]);
        out.insert(at, &line);
        return join(&out);
    }
    let mut out = String::from(source);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&alloc::format!("[{section}]\n{line}\n"));
    out
}

/// Returns `source` without `key` in `section`; the rest stays as it is.
pub fn remove(source: &str, section: &str, key: &str) -> String {
    let mut current = "";
    let mut out: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    let mut removed = false;
    for raw in source.lines() {
        let text = raw.trim();
        if let Some(name) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            current = name.trim();
        } else if current == section
            && !text.starts_with(['#', ';'])
            && text.split_once('=').is_some_and(|(k, _)| k.trim() == key)
        {
            removed = true;
            continue;
        }
        out.push(raw);
    }
    if !removed {
        return String::from(source);
    }
    join(&out)
}

fn join(lines: &[&str]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Quotes a value if it would not survive unquoted (leading or trailing
/// spaces, empty, or starting with a quote).
fn quote(value: &str) -> Cow<'_, str> {
    let plain = !value.is_empty()
        && value.trim() == value
        && !value.starts_with('"')
        && !value.contains(['\n', '\r']);
    if plain {
        return Cow::Borrowed(value);
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use alloc::vec::Vec;

    fn entries(source: &str) -> Vec<Entry<'_>> {
        parse(source).collect::<Result<_, _>>().unwrap()
    }

    #[test]
    fn sections_keys_and_comments() {
        let parsed = entries("# comment\ntop = 1\n\n[display]\n; another\nnight-mode = on\n");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].section, "");
        assert_eq!(parsed[0].key, "top");
        assert_eq!(parsed[0].value.as_i64(), Some(1));
        assert_eq!(parsed[1].section, "display");
        assert_eq!(parsed[1].key, "night-mode");
        assert_eq!(parsed[1].value.as_bool(), Some(true));
        assert_eq!(parsed[1].line, 6);
    }

    #[test]
    fn unquoted_values_are_literal() {
        let parsed = entries("a = Hello # not a comment; \\n stays\n");
        assert_eq!(parsed[0].value.as_str(), "Hello # not a comment; \\n stays");
    }

    #[test]
    fn quoted_values_resolve_escapes() {
        let parsed = entries(r#"a = "  line\nnext \"q\" \\ \u{1F4D6}""#);
        assert_eq!(parsed[0].value.as_str(), "  line\nnext \"q\" \\ \u{1F4D6}");
    }

    #[test]
    fn quoted_value_without_escapes_borrows() {
        let parsed = entries(r#"a = "plain""#);
        assert!(matches!(parsed[0].value.as_str(), Cow::Borrowed("plain")));
    }

    #[test]
    fn errors_report_line_numbers() {
        let error = |source| parse(source).find_map(Result::err).unwrap();
        assert_eq!(error("ok = 1\nbroken").kind, ErrorKind::InvalidLine);
        assert_eq!(error("ok = 1\nbroken").line, 2);
        assert_eq!(error("[open").kind, ErrorKind::InvalidSection);
        assert_eq!(error("bad key = 1").kind, ErrorKind::InvalidKey);
        assert_eq!(error(r#"a = "open"#).kind, ErrorKind::UnterminatedString);
        assert_eq!(error(r#"a = "\x""#).kind, ErrorKind::InvalidEscape);
        assert_eq!(error(r#"a = "\u{zz}""#).kind, ErrorKind::InvalidEscape);
        assert_eq!(error(r#"a = "x" trailing"#).kind, ErrorKind::InvalidLine);
    }

    #[test]
    fn handles_crlf_line_endings() {
        let parsed = entries("[s]\r\nkey = value\r\n");
        assert_eq!(parsed[0].value.as_str(), "value");
    }

    #[test]
    fn remove_keeps_everything_else() {
        let source = "# top
[menus]
a = x, y
# note
b = z
[other]
a = 1
";
        assert_eq!(
            remove(source, "menus", "a"),
            "# top
[menus]
# note
b = z
[other]
a = 1
"
        );
        assert_eq!(remove(source, "menus", "missing"), source);
    }

    #[test]
    fn set_replaces_inserts_and_appends() {
        let source =
            "# My settings\n[core]\nlanguage = en  \n\n# reading\n[reader]\nfont-size = 11\n";
        let replaced = set(source, "core", "language", "de");
        assert_eq!(
            replaced,
            "# My settings\n[core]\nlanguage = de\n\n# reading\n[reader]\nfont-size = 11\n"
        );
        let inserted = set(source, "core", "night-mode", "on");
        assert!(inserted.contains("language = en  \nnight-mode = on\n\n# reading"));
        let appended = set(source, "files", "root", " my books ");
        assert!(appended.ends_with("font-size = 11\n\n[files]\nroot = \" my books \"\n"));
        assert_eq!(set("", "a", "b", "c"), "[a]\nb = c\n");
        // The result parses back to the new values.
        let entry = parse(&appended).last().unwrap().unwrap();
        assert_eq!(entry.value.as_str(), " my books ");
    }
}

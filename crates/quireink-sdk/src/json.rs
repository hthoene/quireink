//! The little JSON web services answer with: flat objects of strings and
//! numbers (e.g. a sync server's, an encyclopedia's summary). Nested
//! values are skipped, or taken as they are written ([`raw_member`]).

pub mod stream;

use alloc::string::String;
use alloc::vec::Vec;

/// A value of a flat object.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    /// `true`, `false`, `null` or a nested object or array.
    Other,
}

/// The members of a JSON object; empty if `text` is none.
pub fn object(text: &str) -> Vec<(String, Value)> {
    let mut p = Parser(text.trim().chars().peekable());
    p.object().unwrap_or_default()
}

/// The string member `key` of an object.
pub fn text<'a>(members: &'a [(String, Value)], key: &str) -> Option<&'a str> {
    members.iter().find_map(|(k, v)| match v {
        Value::Str(s) if k == key => Some(s.as_str()),
        _ => None,
    })
}

/// The number member `key` of an object.
pub fn number(members: &[(String, Value)], key: &str) -> Option<f64> {
    members.iter().find_map(|(k, v)| match v {
        Value::Num(n) if k == key => Some(*n),
        _ => None,
    })
}

/// `text` as a JSON string literal.
pub fn string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&alloc::format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The JSON text of member `key` of the object `text`, as it is written
/// (nested objects and arrays whole), e.g. to keep a value as it came.
pub fn raw_member<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let mut i = skip_space(bytes, 0);
    if bytes.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = skip_space(bytes, i);
        if bytes.get(i) != Some(&b'"') {
            return None;
        }
        let name_end = string_end(bytes, i)?;
        let name = unquote(&text[i..name_end])?;
        i = skip_space(bytes, name_end);
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        let start = skip_space(bytes, i + 1);
        let end = value_end(bytes, start)?;
        if name == key {
            return Some(text[start..end].trim_end());
        }
        i = skip_space(bytes, end);
        match bytes.get(i) {
            Some(b',') => i += 1,
            _ => return None,
        }
    }
}

/// The string a JSON string literal (with its quotes) stands for.
pub fn unquote(literal: &str) -> Option<String> {
    Parser(literal.trim().chars().peekable()).string()
}

/// The strings of a JSON array literal; other items are left out.
pub fn string_array(literal: &str) -> Vec<String> {
    let bytes = literal.as_bytes();
    let mut i = skip_space(bytes, 0);
    let mut out = Vec::new();
    if bytes.get(i) != Some(&b'[') {
        return out;
    }
    i += 1;
    loop {
        i = skip_space(bytes, i);
        let Some(end) = value_end(bytes, i).filter(|&end| end > i) else {
            return out;
        };
        if let Some(text) = unquote(&literal[i..end]) {
            out.push(text);
        }
        i = skip_space(bytes, end);
        match bytes.get(i) {
            Some(b',') => i += 1,
            _ => return out,
        }
    }
}

fn skip_space(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Where the string whose `"` is at `i` ends, after its closing `"`.
fn string_end(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// Where the value starting at `i` ends.
fn value_end(bytes: &[u8], i: usize) -> Option<usize> {
    match bytes.get(i)? {
        b'"' => string_end(bytes, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < bytes.len() {
                match bytes[j] {
                    b'"' => {
                        j = string_end(bytes, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < bytes.len() && !matches!(bytes[j], b',' | b'}' | b']') {
                j += 1;
            }
            Some(j)
        }
    }
}

struct Parser<'a>(core::iter::Peekable<core::str::Chars<'a>>);

impl Parser<'_> {
    fn space(&mut self) {
        while self.0.peek().is_some_and(|c| c.is_whitespace()) {
            self.0.next();
        }
    }

    fn eat(&mut self, c: char) -> Option<()> {
        self.space();
        (self.0.next()? == c).then_some(())
    }

    fn object(&mut self) -> Option<Vec<(String, Value)>> {
        self.eat('{')?;
        let mut members = Vec::new();
        self.space();
        if self.0.peek() == Some(&'}') {
            self.0.next();
            return Some(members);
        }
        loop {
            self.space();
            let key = self.string()?;
            self.eat(':')?;
            let value = self.value()?;
            members.push((key, value));
            self.space();
            match self.0.next()? {
                ',' => {}
                '}' => return Some(members),
                _ => return None,
            }
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.space();
        match *self.0.peek()? {
            '"' => self.string().map(Value::Str),
            '{' | '[' => {
                self.skip_nested()?;
                Some(Value::Other)
            }
            c if c == '-' || c.is_ascii_digit() => {
                let mut number = String::new();
                while let Some(&c) = self.0.peek() {
                    if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E') {
                        number.push(c);
                        self.0.next();
                    } else {
                        break;
                    }
                }
                number.parse().ok().map(Value::Num)
            }
            _ => {
                // true, false, null
                while self.0.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.0.next();
                }
                Some(Value::Other)
            }
        }
    }

    fn skip_nested(&mut self) -> Option<()> {
        let mut depth = 0u32;
        loop {
            match self.0.next()? {
                '{' | '[' => depth += 1,
                '}' | ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(());
                    }
                }
                '"' => loop {
                    match self.0.next()? {
                        '\\' => {
                            self.0.next()?;
                        }
                        '"' => break,
                        _ => {}
                    }
                },
                _ => {}
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        if self.0.next()? != '"' {
            return None;
        }
        let mut out = String::new();
        loop {
            match self.0.next()? {
                '"' => return Some(out),
                '\\' => match self.0.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let mut code = self.hex4()?;
                        // A surrogate pair.
                        if (0xd800..0xdc00).contains(&code) {
                            if self.0.next()? != '\\' || self.0.next()? != 'u' {
                                return None;
                            }
                            let low = self.hex4()?;
                            code = 0x10000 + ((code - 0xd800) << 10) + low.wrapping_sub(0xdc00);
                        }
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    c => out.push(c),
                },
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let mut value = 0;
        for _ in 0..4 {
            value = value * 16 + self.0.next()?.to_digit(16)?;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_flat_objects() {
        let o = object(
            r#"{"document":"abc","percentage":0.45,"progress":"/body/DocFragment[3]/body/p[2]",
               "device":"KöReader \"Kobo\"","timestamp":1700000000,"x":{"y":[1,"}"]},"ok":true}"#,
        );
        assert_eq!(text(&o, "document"), Some("abc"));
        assert_eq!(number(&o, "percentage"), Some(0.45));
        assert_eq!(text(&o, "device"), Some("KöReader \"Kobo\""));
        assert_eq!(number(&o, "timestamp"), Some(1_700_000_000.0));
        assert_eq!(text(&o, "missing"), None);
        assert!(object("{}").is_empty());
        assert!(object("not json").is_empty());
    }

    #[test]
    fn raw_members_whole() {
        let o = r#"{"lpath": "A \"B\".epub", "n": 12 , "meta": {"t": "x}", "a": ["y", {"z": "]"}]},
            "list": ["one", 2, "tw\u00f6"], "last": true}"#;
        assert_eq!(raw_member(o, "n"), Some("12"));
        assert_eq!(
            raw_member(o, "lpath").and_then(unquote).as_deref(),
            Some("A \"B\".epub")
        );
        let meta = raw_member(o, "meta").unwrap();
        assert!(meta.starts_with('{') && meta.ends_with('}'));
        assert_eq!(
            raw_member(meta, "t").and_then(unquote).as_deref(),
            Some("x}")
        );
        assert_eq!(string_array(raw_member(o, "list").unwrap()), ["one", "twö"]);
        assert_eq!(raw_member(o, "last"), Some("true"));
        assert_eq!(raw_member(o, "missing"), None);
        assert_eq!(raw_member("[1]", "x"), None);
        assert!(string_array("{}").is_empty());
    }

    #[test]
    fn writes_strings() {
        assert_eq!(string("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }
}

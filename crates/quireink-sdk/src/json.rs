//! The little JSON web services answer with: flat objects of strings and
//! numbers (e.g. a sync server's, an encyclopedia's summary). Nested
//! values are skipped.

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
    fn writes_strings() {
        assert_eq!(string("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }
}

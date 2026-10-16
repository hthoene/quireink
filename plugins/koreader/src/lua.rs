//! Lua values as KOReader keeps its settings: a file `return { … }` of
//! nested tables, strings, numbers and booleans, as its `dump` writes
//! them. Read in full and written back in the same form, so what
//! QuireInk does not know of an annotation stays as it was.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// A Lua value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    /// As written, e.g. `12` or `0.5`.
    Number(String),
    Str(String),
    Table(Table),
}

/// A table: its fields in order, with their keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub fields: Vec<(Value, Value)>,
}

impl Table {
    /// The value at string key `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
            .map(|(_, v)| v)
    }

    /// The string at `key`, if it is one.
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Sets string key `key` to `value`, in place or at the end; `Nil`
    /// removes it.
    pub fn set(&mut self, key: &str, value: Value) {
        let at = self
            .fields
            .iter()
            .position(|(k, _)| matches!(k, Value::Str(s) if s == key));
        match (at, value) {
            (Some(at), Value::Nil) => {
                self.fields.remove(at);
            }
            (Some(at), value) => self.fields[at].1 = value,
            (None, Value::Nil) => {}
            (None, value) => self.fields.push((Value::Str(String::from(key)), value)),
        }
    }

    /// The values at keys 1, 2, … (a list).
    pub fn list(&self) -> impl Iterator<Item = &Value> {
        let mut items: Vec<(u64, &Value)> = self
            .fields
            .iter()
            .filter_map(|(k, v)| match k {
                Value::Number(n) => n.parse::<u64>().ok().map(|n| (n, v)),
                _ => None,
            })
            .collect();
        items.sort_by_key(|(n, _)| *n);
        items.into_iter().map(|(_, v)| v)
    }

    /// A list of `items` (keys 1, 2, …).
    pub fn from_list(items: impl IntoIterator<Item = Value>) -> Self {
        Self {
            fields: items
                .into_iter()
                .enumerate()
                .map(|(i, v)| (Value::Number(alloc::format!("{}", i + 1)), v))
                .collect(),
        }
    }
}

/// Reads a settings file: comments, then `return` and a value.
pub fn parse(text: &str) -> Option<Value> {
    let mut parser = Parser {
        text: text.as_bytes(),
        at: 0,
        depth: 0,
    };
    parser.blank();
    if parser.text[parser.at..].starts_with(b"return") {
        parser.at += 6;
    }
    let value = parser.value()?;
    parser.blank();
    (parser.at == parser.text.len()).then_some(value)
}

/// Deepest nesting read, against files made to exhaust the stack.
const MAX_DEPTH: u32 = 64;

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
    depth: u32,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    /// Skips white space and comments.
    fn blank(&mut self) {
        loop {
            while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
                self.at += 1;
            }
            if !self.text[self.at..].starts_with(b"--") {
                return;
            }
            self.at += 2;
            if let Some(end) = self.long_bracket() {
                self.at = end;
                continue;
            }
            while self.peek().is_some_and(|b| b != b'\n') {
                self.at += 1;
            }
        }
    }

    /// At `[[` or `[==[`: the end of the long bracket's content and its
    /// closing, as `(content end, after)`; moves past the opening.
    fn long_open(&mut self) -> Option<(usize, usize)> {
        let rest = &self.text[self.at..];
        let level = rest.iter().skip(1).take_while(|&&b| b == b'=').count();
        if rest.first() != Some(&b'[') || rest.get(level + 1) != Some(&b'[') {
            return None;
        }
        let mut close = Vec::with_capacity(level + 2);
        close.push(b']');
        close.extend(core::iter::repeat_n(b'=', level));
        close.push(b']');
        let start = self.at + level + 2;
        let end = self.text[start..]
            .windows(close.len())
            .position(|w| w == close.as_slice())?
            + start;
        self.at = start;
        Some((end, end + close.len()))
    }

    /// A long comment's end, if one begins here.
    fn long_bracket(&mut self) -> Option<usize> {
        self.long_open().map(|(_, after)| after)
    }

    fn value(&mut self) -> Option<Value> {
        self.blank();
        match self.peek()? {
            b'{' => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return None;
                }
                let table = self.table();
                self.depth -= 1;
                table.map(Value::Table)
            }
            b'"' | b'\'' => self.string().map(Value::Str),
            b'[' => {
                let (end, after) = self.long_open()?;
                let mut text = &self.text[self.at..end];
                // A line break right after the opening is not part of it.
                if text.first() == Some(&b'\n') {
                    text = &text[1..];
                }
                let value = String::from_utf8_lossy(text).into_owned();
                self.at = after;
                Some(Value::Str(value))
            }
            b'-' | b'.' | b'0'..=b'9' => self.number(),
            _ => {
                let word = self.word();
                match word.as_str() {
                    "nil" => Some(Value::Nil),
                    "true" => Some(Value::Bool(true)),
                    "false" => Some(Value::Bool(false)),
                    _ => None,
                }
            }
        }
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.text[start..self.at]).into_owned()
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        while self.peek().is_some_and(|b| {
            b.is_ascii_hexdigit() || matches!(b, b'.' | b'x' | b'X' | b'+' | b'-' | b'p' | b'P')
        }) {
            // A sign only after an exponent.
            if matches!(self.peek(), Some(b'+' | b'-'))
                && !matches!(self.text.get(self.at - 1), Some(b'e' | b'E' | b'p' | b'P'))
            {
                break;
            }
            self.at += 1;
        }
        let text = core::str::from_utf8(&self.text[start..self.at]).ok()?;
        (text != "-" && !text.is_empty()).then(|| Value::Number(String::from(text)))
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.peek()?;
        self.at += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let b = self.peek()?;
            self.at += 1;
            match b {
                b'\\' => {
                    let e = self.peek()?;
                    self.at += 1;
                    match e {
                        b'n' | b'\n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'a' => out.push(7),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'v' => out.push(11),
                        b'x' => {
                            let hex = self.text.get(self.at..self.at + 2)?;
                            let hex = core::str::from_utf8(hex).ok()?;
                            out.push(u8::from_str_radix(hex, 16).ok()?);
                            self.at += 2;
                        }
                        b'z' => {
                            while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
                                self.at += 1;
                            }
                        }
                        b'0'..=b'9' => {
                            let mut n = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'9') => {
                                        n = n * 10 + u32::from(d - b'0');
                                        self.at += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(u8::try_from(n).ok()?);
                        }
                        other => out.push(other),
                    }
                }
                b'\n' => return None,
                b if b == quote => break,
                b => out.push(b),
            }
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    fn table(&mut self) -> Option<Table> {
        self.at += 1;
        let mut table = Table::default();
        let mut next = 1u64;
        loop {
            self.blank();
            match self.peek()? {
                b'}' => {
                    self.at += 1;
                    return Some(table);
                }
                b',' | b';' => {
                    self.at += 1;
                    continue;
                }
                _ => {}
            }
            let key;
            if self.peek() == Some(b'[') && self.text.get(self.at + 1) != Some(&b'[') {
                self.at += 1;
                key = self.value()?;
                self.blank();
                if self.peek()? != b']' {
                    return None;
                }
                self.at += 1;
                self.blank();
                if self.peek()? != b'=' {
                    return None;
                }
                self.at += 1;
            } else {
                // `name = value`, or a value in the list.
                let start = self.at;
                let word = self.word();
                self.blank();
                if !word.is_empty()
                    && !matches!(word.as_str(), "nil" | "true" | "false")
                    && self.peek() == Some(b'=')
                {
                    self.at += 1;
                    key = Value::Str(word);
                } else {
                    self.at = start;
                    key = Value::Number(alloc::format!("{next}"));
                    next += 1;
                }
            }
            let value = self.value()?;
            if value != Value::Nil {
                table.fields.push((key, value));
            }
        }
    }
}

/// `value` as KOReader writes settings, ready to be read back:
/// `return` and the value, tables with a field a line.
pub fn write(value: &Value) -> String {
    let mut out = String::from("-- we can read Lua syntax here!\nreturn ");
    write_value(value, 0, &mut out);
    out.push('\n');
    out
}

fn write_value(value: &Value, indent: usize, out: &mut String) {
    match value {
        Value::Nil => out.push_str("nil"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(n),
        Value::Str(s) => quote(s, out),
        Value::Table(table) => {
            out.push('{');
            for (key, value) in &table.fields {
                out.push('\n');
                pad(indent + 1, out);
                out.push('[');
                write_value(key, indent + 1, out);
                out.push_str("] = ");
                write_value(value, indent + 1, out);
                out.push(',');
            }
            if !table.fields.is_empty() {
                out.push('\n');
                pad(indent, out);
            }
            out.push('}');
        }
    }
}

fn pad(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("    ");
    }
}

/// A Lua string literal.
fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\{:03}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A table value, for building them.
pub fn table(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Table(Table {
        fields: fields
            .into_iter()
            .filter(|(_, v)| *v != Value::Nil)
            .map(|(k, v)| (Value::Str(String::from(k)), v))
            .collect(),
    })
}

/// A string value.
pub fn string(s: &str) -> Value {
    Value::Str(String::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDECAR: &str = r#"-- we can read Lua syntax here!
return {
    ["annotations"] = {
        [1] = {
            ["chapter"] = "Chapter I",
            ["datetime"] = "2026-10-01 20:15:03",
            ["drawer"] = "lighten",
            ["note"] = "Two\
lines",
            ["page"] = "/body/DocFragment[3]/body/p[4]/text().0",
            ["pos0"] = "/body/DocFragment[3]/body/p[4]/text().0",
            ["pos1"] = "/body/DocFragment[3]/body/p[4]/text().12",
            ["text"] = "Down the \"Rabbit\" Hole",
        },
        [2] = {
            ["datetime"] = "2026-10-02 08:00:00",
            ["page"] = "/body/DocFragment[5]/body/p[1]/text().0",
        },
    },
    ["percent_finished"] = 0.25,
    ["summary"] = {
        status = 'reading', -- a comment
        modified = [[2026-10-02]],
    },
    ["stats"] = { 1, 2, -3.5e-2, true, false, nil, 0x1F },
}
"#;

    #[test]
    fn reads_koreaders_settings() {
        let Some(Value::Table(root)) = parse(SIDECAR) else {
            panic!("not read");
        };
        let Some(Value::Table(annotations)) = root.get("annotations") else {
            panic!("no annotations");
        };
        let items: Vec<&Value> = annotations.list().collect();
        assert_eq!(items.len(), 2);
        let Value::Table(first) = items[0] else {
            panic!()
        };
        assert_eq!(first.str("note"), Some("Two\nlines"));
        assert_eq!(first.str("text"), Some("Down the \"Rabbit\" Hole"));
        assert_eq!(
            root.get("percent_finished"),
            Some(&Value::Number(String::from("0.25")))
        );
        let Some(Value::Table(summary)) = root.get("summary") else {
            panic!()
        };
        assert_eq!(summary.str("status"), Some("reading"));
        assert_eq!(summary.str("modified"), Some("2026-10-02"));
        let Some(Value::Table(stats)) = root.get("stats") else {
            panic!()
        };
        // nil leaves no field, but takes its place in the list.
        assert_eq!(stats.fields.len(), 6);
        assert_eq!(stats.fields[5].1, Value::Number(String::from("0x1F")));
    }

    #[test]
    fn written_as_read() {
        let value = parse(SIDECAR).unwrap();
        let again = parse(&write(&value)).unwrap();
        assert_eq!(again, value);
        let text = write(&table([("text", string("a\"b\\c\nd\u{1}"))]));
        assert!(text.contains(r#"["text"] = "a\"b\\c\nd\001","#));
    }

    #[test]
    fn nonsense_is_refused() {
        for text in [
            "",
            "return {",
            "return {[1] 2}",
            "return \"open",
            "return nonsense",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
        let deep = alloc::format!("return {}{}", "{".repeat(100), "}".repeat(100));
        assert_eq!(parse(&deep), None);
    }
}

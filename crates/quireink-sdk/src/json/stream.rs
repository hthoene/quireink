//! Reading JSON as it streams, e.g. from a file, with little memory: a
//! [`Parser`] reports each value with its path ([`Event`]), and the
//! caller keeps only what it needs. Strings are cut at [`MAX_STRING`]
//! bytes.
//!
//! ```
//! use quireink_sdk::json::stream::{Event, Parser};
//!
//! let mut titles = Vec::new();
//! let mut parser = Parser::default();
//! parser.feed(br#"{"items": [{"title": "A"}, {"title": "B"}]}"#, &mut |event| {
//!     if let Event::Value(path, text) = event
//!         && path == ["items", "[]", "title"]
//!     {
//!         titles.push(text.to_string());
//!     }
//! });
//! assert_eq!(titles, ["A", "B"]);
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use quireink_platform::Storage;

/// Pieces a file is read in.
const CHUNK: usize = 2048;
/// Strings are cut at this many bytes.
pub const MAX_STRING: usize = 4096;

/// Where a value is: the keys from the top, `[]` for array items.
pub type Path = Vec<String>;

/// What the parser reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// An object or array starts at this path.
    Open(&'a [String]),
    /// It ends.
    Close(&'a [String]),
    /// A string (or other plain value, as text) at this path.
    Value(&'a [String], &'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Object,
    Array,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Between,
    /// In a string: its bytes, whether it is a key, and an escape seen.
    Text {
        key: bool,
        escape: bool,
        /// The hex digits of a `\u` escape so far.
        unicode: Option<String>,
    },
    /// In a number or `true`/`false`/`null`.
    Plain,
}

/// A streaming JSON parser that reports values with their paths.
#[derive(Debug, Default)]
pub struct Parser {
    path: Path,
    containers: Vec<Container>,
    /// The key of the next value in an object.
    key: Option<String>,
    state: State,
    bytes: Vec<u8>,
    /// A high surrogate waiting for its low half.
    surrogate: Option<u16>,
}

impl Parser {
    fn value_path(&self) -> Path {
        let mut path = self.path.clone();
        match self.containers.last() {
            Some(Container::Array) => path.push(String::from("[]")),
            Some(Container::Object) => path.push(self.key.clone().unwrap_or_default()),
            None => {}
        }
        path
    }

    /// Reads `data`, the next piece of the text, reporting what it finds.
    pub fn feed(&mut self, data: &[u8], emit: &mut dyn FnMut(Event<'_>)) {
        for &b in data {
            self.byte(b, emit);
        }
    }

    fn push_char(&mut self, c: char) {
        if self.bytes.len() < MAX_STRING {
            let mut buf = [0u8; 4];
            self.bytes
                .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }

    fn end_plain(&mut self, emit: &mut dyn FnMut(Event<'_>)) {
        let text = String::from_utf8_lossy(&self.bytes).into_owned();
        let path = self.value_path();
        emit(Event::Value(&path, &text));
        self.bytes.clear();
        self.state = State::Between;
    }

    fn byte(&mut self, b: u8, emit: &mut dyn FnMut(Event<'_>)) {
        match &mut self.state {
            State::Text {
                key,
                escape,
                unicode,
            } => {
                if let Some(hex) = unicode {
                    hex.push(char::from(b));
                    if hex.len() == 4 {
                        let unit = u16::from_str_radix(hex, 16).unwrap_or(0xfffd);
                        *unicode = None;
                        *escape = false;
                        match (self.surrogate.take(), unit) {
                            (None, 0xd800..=0xdbff) => self.surrogate = Some(unit),
                            (Some(high), 0xdc00..=0xdfff) => {
                                let c = 0x10000
                                    + ((u32::from(high) - 0xd800) << 10)
                                    + (u32::from(unit) - 0xdc00);
                                self.push_char(char::from_u32(c).unwrap_or('\u{fffd}'));
                            }
                            (_, unit) => self
                                .push_char(char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}')),
                        }
                    }
                    return;
                }
                if *escape {
                    *escape = false;
                    let c = match b {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' | b'f' => ' ',
                        b'u' => {
                            *unicode = Some(String::new());
                            return;
                        }
                        other => char::from(other),
                    };
                    self.push_char(c);
                    return;
                }
                match b {
                    b'\\' => *escape = true,
                    b'"' => {
                        let is_key = *key;
                        let text = String::from_utf8_lossy(&self.bytes).into_owned();
                        self.bytes.clear();
                        self.state = State::Between;
                        if is_key {
                            self.key = Some(text);
                        } else {
                            let path = self.value_path();
                            emit(Event::Value(&path, &text));
                        }
                    }
                    _ => {
                        if self.bytes.len() < MAX_STRING {
                            self.bytes.push(b);
                        }
                    }
                }
            }
            State::Plain => {
                if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+') {
                    self.bytes.push(b);
                } else {
                    self.end_plain(emit);
                    self.byte(b, emit);
                }
            }
            State::Between => match b {
                b'{' | b'[' => {
                    let path = self.value_path();
                    emit(Event::Open(&path));
                    self.path = path;
                    self.containers.push(if b == b'{' {
                        Container::Object
                    } else {
                        Container::Array
                    });
                    self.key = None;
                }
                b'}' | b']' => {
                    self.containers.pop();
                    let path = core::mem::take(&mut self.path);
                    emit(Event::Close(&path));
                    let mut parent = path;
                    let last = parent.pop();
                    self.path = parent;
                    // Back in an object: the key it was under.
                    self.key = match self.containers.last() {
                        Some(Container::Object) => last,
                        _ => None,
                    };
                }
                b'"' => {
                    let key =
                        self.containers.last() == Some(&Container::Object) && self.key.is_none();
                    self.state = State::Text {
                        key,
                        escape: false,
                        unicode: None,
                    };
                }
                b',' => {
                    if self.containers.last() == Some(&Container::Object) {
                        self.key = None;
                    }
                }
                b':' => {}
                b if b.is_ascii_whitespace() => {}
                b => {
                    self.state = State::Plain;
                    self.bytes.push(b);
                }
            },
        }
    }
}

/// Reads the JSON file at `path`, a piece at a time, reporting what it
/// finds to `emit`.
pub fn read_file(storage: &dyn Storage, path: &str, emit: &mut dyn FnMut(Event<'_>)) {
    let mut parser = Parser::default();
    let mut buf = [0u8; CHUNK];
    let mut offset = 0;
    while let Ok(n) = storage.read_at(path, offset, &mut buf) {
        if n == 0 {
            break;
        }
        parser.feed(&buf[..n], emit);
        offset += n as u64;
    }
}

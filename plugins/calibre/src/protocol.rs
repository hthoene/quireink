//! Calibre's wireless device protocol, as its driver speaks it
//! (`calibre/devices/smart_device_app/driver.py`): messages are JSON
//! arrays `[opcode, {arguments}]`, each written after its length in
//! decimal digits; a book's bytes follow its `SEND_BOOK` message as they
//! are. Calibre is found by a UDP datagram to one of its broadcast ports;
//! it answers with its name and the port to connect to.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::json;

/// The ports Calibre listens on for a device looking for it.
pub const BROADCAST_PORTS: [u16; 5] = [54982, 48123, 39001, 44044, 59678];
/// The port Calibre takes for devices unless set otherwise.
pub const DEFAULT_PORT: u16 = 9090;

/// The opcodes QuireInk knows.
pub mod op {
    pub const OK: u8 = 0;
    pub const SET_CALIBRE_DEVICE_INFO: u8 = 1;
    pub const SET_CALIBRE_DEVICE_NAME: u8 = 2;
    pub const GET_DEVICE_INFORMATION: u8 = 3;
    pub const TOTAL_SPACE: u8 = 4;
    pub const FREE_SPACE: u8 = 5;
    pub const GET_BOOK_COUNT: u8 = 6;
    pub const SEND_BOOKLISTS: u8 = 7;
    pub const SEND_BOOK: u8 = 8;
    pub const GET_INITIALIZATION_INFO: u8 = 9;
    pub const BOOK_DONE: u8 = 11;
    pub const NOOP: u8 = 12;
    pub const DELETE_BOOK: u8 = 13;
    pub const GET_BOOK_FILE_SEGMENT: u8 = 14;
    pub const GET_BOOK_METADATA: u8 = 15;
    pub const SEND_BOOK_METADATA: u8 = 16;
    pub const DISPLAY_MESSAGE: u8 = 17;
    pub const CALIBRE_BUSY: u8 = 18;
    pub const SET_LIBRARY_INFO: u8 = 19;
    pub const ERROR: u8 = 20;
}

/// A message: its opcode and its arguments as JSON (an object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub op: u8,
    pub args: String,
}

/// `op` with `args` (a JSON object) as it goes on the connection.
pub fn frame(op: u8, args: &str) -> Vec<u8> {
    let body = format!("[{op}, {args}]");
    format!("{}{body}", body.len()).into_bytes()
}

/// Longest message read (Calibre's are small; a book's metadata with its
/// description is the largest).
const MAX_MESSAGE: usize = 1 << 20;

/// Reads messages from what comes, never asking for more than the
/// current message holds, so a book's bytes after it are not taken as
/// text.
#[derive(Debug, Default)]
pub struct Framing {
    /// The digits of the length so far.
    digits: String,
    /// The message's text so far, once its length is known.
    body: Option<(usize, String)>,
}

impl Framing {
    /// How many bytes to read next.
    pub fn wanted(&self) -> u32 {
        match &self.body {
            // A digit or the `[` at a time.
            None => 1,
            Some((len, text)) => (len - text.len()).max(1) as u32,
        }
    }

    /// Takes what came; a message when it is complete. `Err` for what
    /// is not Calibre's protocol.
    pub fn feed(&mut self, text: &str) -> Result<Option<Message>, &'static str> {
        for c in text.chars() {
            match &mut self.body {
                None if c.is_ascii_digit() => {
                    self.digits.push(c);
                    if self.digits.len() > 8 {
                        return Err("length too long");
                    }
                }
                None if c == '[' => {
                    let len: usize = self.digits.parse().map_err(|_| "no length")?;
                    if len == 0 || len > MAX_MESSAGE {
                        return Err("bad length");
                    }
                    self.digits.clear();
                    self.body = Some((len, String::from("[")));
                }
                None => return Err("not a message"),
                Some((_, body)) => body.push(c),
            }
        }
        match &self.body {
            Some((len, body)) if body.len() >= *len => {
                let (_, body) = self.body.take().unwrap_or_default();
                parse(&body).map(Some).ok_or("bad message")
            }
            _ => Ok(None),
        }
    }
}

/// `[op, {...}]` as a message.
pub fn parse(body: &str) -> Option<Message> {
    let inner = body.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (op, args) = inner.split_once(',')?;
    Some(Message {
        op: op.trim().parse().ok()?,
        args: args.trim().to_string(),
    })
}

/// Member `key` of `object`, as it is written.
pub fn member<'a>(object: &'a str, key: &str) -> Option<&'a str> {
    json::raw_member(object, key)
}

/// Member `key` of `object` as text: a JSON string decoded, other
/// values as written.
pub fn text(object: &str, key: &str) -> Option<String> {
    let raw = member(object, key)?;
    json::unquote(raw).or_else(|| Some(raw.to_string()))
}

/// Member `key` of `object` as a number.
pub fn number(object: &str, key: &str) -> Option<u64> {
    member(object, key)?.parse::<f64>().ok().map(|n| n as u64)
}

/// The strings of the array member `key` of `object`.
pub fn strings(object: &str, key: &str) -> Vec<String> {
    member(object, key)
        .map(json::string_array)
        .unwrap_or_default()
}

/// What the device says of itself when Calibre asks
/// (`GET_INITIALIZATION_INFO`).
#[derive(Debug)]
pub struct Device<'a> {
    pub name: &'a str,
    /// The file extensions QuireInk reads.
    pub extensions: &'a [&'a str],
    /// Calibre's challenge and the password, if one is set.
    pub challenge: &'a str,
    pub password: Option<&'a str>,
    pub version: &'a str,
}

/// The answer to `GET_INITIALIZATION_INFO`.
pub fn initialization(device: &Device<'_>) -> String {
    let extensions: Vec<String> = device.extensions.iter().map(|e| json::string(e)).collect();
    let hash = match device.password {
        Some(password) if !device.challenge.is_empty() => {
            let mut sha = quireink_sdk::sha1::Sha1::new();
            sha.update(password.as_bytes());
            sha.update(device.challenge.as_bytes());
            sha.hex()
        }
        _ => String::new(),
    };
    format!(
        "{{\"versionOK\": true, \"maxBookContentPacketLen\": 65536, \
         \"acceptedExtensions\": [{extensions}], \"extensionPathLengths\": {{}}, \
         \"canStreamBooks\": true, \"canStreamMetadata\": true, \
         \"canReceiveBookBinary\": true, \"canDeleteMultipleBooks\": true, \
         \"canUseCachedMetadata\": false, \"cacheUsesLpaths\": true, \
         \"canSendOkToSendbook\": false, \"canAcceptLibraryInfo\": true, \
         \"willAskForUpdateBooks\": false, \"useUuidFileNames\": false, \
         \"coverHeight\": 240, \"deviceKind\": \"QuireInk\", \"deviceName\": {name}, \
         \"appName\": \"QuireInk\", \"ccVersionNumber\": {version}, \
         \"passwordHash\": {hash}}}",
        extensions = extensions.join(", "),
        name = json::string(device.name),
        version = json::string(device.version),
        hash = json::string(&hash),
    )
}

/// The reply to Calibre's `GET_DEVICE_INFORMATION`: what Calibre told
/// the device about itself last (`SET_CALIBRE_DEVICE_INFO`), if anything.
pub fn device_information(stored: Option<&str>, name: &str, version: &str) -> String {
    let info = stored
        .filter(|s| s.trim_start().starts_with('{'))
        .map_or_else(
            || format!("{{\"device_name\": {}}}", json::string(name)),
            ToString::to_string,
        );
    format!(
        "{{\"device_info\": {info}, \"version\": {v}, \"device_version\": {v}}}",
        v = json::string(version)
    )
}

/// The path a book is to have below the library folder: Calibre's
/// `lpath`, with no way out of the folder.
pub fn safe_path(lpath: &str) -> Option<String> {
    let parts: Vec<&str> = lpath
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() || parts.iter().any(|p| *p == ".." || p.contains(':')) {
        return None;
    }
    Some(parts.join("/"))
}

/// Calibre's answer to a broadcast: the port to connect to.
pub fn port_of_answer(answer: &str) -> Option<u16> {
    if !answer.starts_with("calibre wireless device client") {
        return None;
    }
    let (_, ports) = answer.rsplit_once(';')?;
    ports.rsplit(',').next()?.trim().parse().ok()
}

/// The name Calibre gives in its answer, e.g. `pc`.
pub fn name_of_answer(answer: &str) -> Option<&str> {
    let start = answer.find("(on ")? + 4;
    let end = answer[start..].find(')')? + start;
    Some(&answer[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_carry_their_length() {
        assert_eq!(frame(op::OK, "{}"), b"7[0, {}]");
        let message = r#"[9, {"passwordChallenge": "", "validExtensions": ["epub"]}]"#;
        let mut framing = Framing::default();
        let mut got = None;
        // Byte by byte, as the screen asks for them.
        let wire = format!("{}{message}", message.len());
        let mut at = 0;
        while got.is_none() {
            let n = framing.wanted() as usize;
            let chunk = &wire[at..(at + n).min(wire.len())];
            at += chunk.len();
            got = framing.feed(chunk).unwrap();
        }
        assert_eq!(at, wire.len(), "read past the message");
        let message = got.unwrap();
        assert_eq!(message.op, op::GET_INITIALIZATION_INFO);
        assert_eq!(
            text(&message.args, "passwordChallenge").as_deref(),
            Some("")
        );
        assert_eq!(strings(&message.args, "validExtensions"), ["epub"]);
        assert!(Framing::default().feed("x").is_err());
        assert!(Framing::default().feed("0[").is_err());
    }

    #[test]
    fn members_whole_and_decoded() {
        let args = r#"{"lpath": "Ann Author/A \"Book\".epub", "length": 1234,
            "metadata": {"title": "A {Book}", "authors": ["Ann", "Bo"], "x": [1, {"y": "]"}]},
            "thisBook": 0}"#;
        assert_eq!(
            text(args, "lpath").as_deref(),
            Some("Ann Author/A \"Book\".epub")
        );
        assert_eq!(number(args, "length"), Some(1234));
        let metadata = member(args, "metadata").unwrap();
        assert!(metadata.starts_with('{') && metadata.ends_with('}'));
        assert_eq!(text(metadata, "title").as_deref(), Some("A {Book}"));
        assert_eq!(strings(metadata, "authors"), ["Ann", "Bo"]);
        assert_eq!(number(args, "thisBook"), Some(0));
        assert_eq!(member(args, "missing"), None);
    }

    #[test]
    fn password_answers_the_challenge() {
        let device = Device {
            name: "QuireInk",
            extensions: &["epub", "txt"],
            challenge: "2026-10-16T12:00:00",
            password: Some("secret"),
            version: "1",
        };
        let answer = initialization(&device);
        let expected = quireink_sdk::sha1::hex(b"secret2026-10-16T12:00:00");
        assert_eq!(text(&answer, "passwordHash"), Some(expected));
        assert_eq!(strings(&answer, "acceptedExtensions"), ["epub", "txt"]);
        assert_eq!(text(&answer, "canStreamBooks").as_deref(), Some("true"));
        let no_password = Device {
            password: None,
            ..device
        };
        assert_eq!(
            text(&initialization(&no_password), "passwordHash").as_deref(),
            Some("")
        );
    }

    #[test]
    fn device_information_returns_what_calibre_said() {
        let stored = r#"{"device_store_uuid": "abc", "device_name": "QuireInk"}"#;
        let info = device_information(Some(stored), "Boox", "1");
        assert_eq!(member(&info, "device_info"), Some(stored));
        let fresh = device_information(None, "Boox", "1");
        assert_eq!(
            text(member(&fresh, "device_info").unwrap(), "device_name").as_deref(),
            Some("Boox")
        );
    }

    #[test]
    fn paths_stay_inside() {
        assert_eq!(safe_path("Ann/Book.epub").as_deref(), Some("Ann/Book.epub"));
        assert_eq!(
            safe_path("\\Ann\\.\\Book.epub").as_deref(),
            Some("Ann/Book.epub")
        );
        assert_eq!(safe_path("../etc/passwd"), None);
        assert_eq!(safe_path("C:/x.epub"), None);
        assert_eq!(safe_path(""), None);
    }

    #[test]
    fn broadcast_answers() {
        let answer = "calibre wireless device client (on pc);8080,9090";
        assert_eq!(port_of_answer(answer), Some(9090));
        assert_eq!(name_of_answer(answer), Some("pc"));
        assert_eq!(
            port_of_answer("calibre wireless device client (on pc);,9091"),
            Some(9091)
        );
        assert_eq!(port_of_answer("hello"), None);
    }
}

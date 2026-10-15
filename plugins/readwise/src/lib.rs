//! Readwise: the highlights of a book go to Readwise when its notes are
//! exported (*Export as Markdown* at the end of a book's bookmarks and
//! highlights), with the notes on them and where they are in the book.
//! Readwise keeps each highlight once, so exporting again sends only
//! what is new.
//!
//! Readwise needs one's access token (readwise.io/access_token): the
//! first export asks for it on the keyboard (or a phone, where offered);
//! it is kept with the other passwords and keys (see
//! `quireink_sdk::secrets`). Shipped switched off: switch it on in the
//! plugins.
//!
//! It reads the Markdown the reader writes (see [`parse`]), so it works
//! with any reader that writes the same and emits
//! `reader.notes-exported`.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{params, slots};
use quireink_sdk::secrets::SecretChange;
use quireink_sdk::{
    Arg, Command, Env, HttpRequest, Id, Manifest, Params, Plugin, RegisterError, Registrar,
    SDK_VERSION, Version, http_params, json,
};

pub const PLUGIN: Id = Id::new_static("readwise");
/// The access token, among the passwords and keys.
pub const SECRET: &str = "readwise.token";
/// The reader's event after notes were exported: parameter `path`, the
/// Markdown file.
const EXPORTED: Id = Id::new_static("reader.notes-exported");
/// The token, entered on the keyboard; parameter `path` goes along.
const TOKEN_ENTERED: Id = Id::new_static("readwise.token-entered");
/// Readwise's answer; parameter `count`, the highlights sent.
const SENT: Id = Id::new_static("readwise.sent");
const COUNT: &str = "count";
/// Longest notes file read.
const MAX_FILE: usize = 256 * 1024;
const URL: &str = "https://readwise.io/api/v2/highlights/";

/// A highlight as the Markdown has it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Highlight {
    pub text: String,
    /// Where in the book, in percent.
    pub percent: u32,
    pub note: String,
}

/// A book's notes as the Markdown has them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Notes {
    pub title: String,
    pub author: String,
    pub highlights: Vec<Highlight>,
}

/// Reads the Markdown the reader writes: `# Title`, the author on the
/// next line, then under the first `##` heading each highlight as a
/// quote (`> text`) ending in its place (`> 12 %`), followed by the note
/// on it, if any; later headings (bookmarks) are left out.
pub fn parse(text: &str) -> Notes {
    let mut notes = Notes::default();
    let mut lines = text.lines().peekable();
    // The title and author, before the first heading of a part.
    while let Some(line) = lines.peek().copied() {
        if line.starts_with("## ") {
            break;
        }
        lines.next();
        if let Some(title) = line.strip_prefix("# ") {
            notes.title = title.trim().to_string();
        } else if !line.trim().is_empty() && notes.author.is_empty() && !notes.title.is_empty() {
            notes.author = line.trim().to_string();
        }
    }
    // The highlights, under the first heading.
    lines.next();
    let mut current: Option<Highlight> = None;
    let mut quoting = false;
    for line in lines {
        if line.starts_with("## ") {
            break;
        }
        if let Some(quote) = line.strip_prefix('>') {
            let quote = quote.trim();
            if !quoting {
                notes.highlights.extend(current.take());
                current = Some(Highlight::default());
                quoting = true;
            }
            let Some(h) = current.as_mut() else { continue };
            if let Some(percent) = quote.strip_suffix('%').and_then(|p| p.trim().parse().ok()) {
                h.percent = percent;
            } else if !quote.is_empty() {
                if !h.text.is_empty() {
                    h.text.push('\n');
                }
                h.text.push_str(quote);
            }
            continue;
        }
        quoting = false;
        if let Some(h) = current.as_mut()
            && !line.trim().is_empty()
        {
            if !h.note.is_empty() {
                h.note.push('\n');
            }
            h.note.push_str(line.trim());
        }
    }
    notes.highlights.extend(current);
    notes.highlights.retain(|h| !h.text.is_empty());
    notes
}

/// The request sending `notes` with `token`.
pub fn request(notes: &Notes, token: &str) -> HttpRequest {
    let mut body = String::from("{\"highlights\":[");
    for (n, h) in notes.highlights.iter().enumerate() {
        if n > 0 {
            body.push(',');
        }
        body.push_str(&format!(
            "{{\"text\":{},\"title\":{},\"source_type\":\"quireink\",\"category\":\"books\",\
             \"location\":{},\"location_type\":\"order\"",
            json::string(&h.text),
            json::string(&notes.title),
            h.percent
        ));
        if !notes.author.is_empty() {
            body.push_str(&format!(",\"author\":{}", json::string(&notes.author)));
        }
        if !h.note.is_empty() {
            body.push_str(&format!(",\"note\":{}", json::string(&h.note)));
        }
        body.push('}');
    }
    body.push_str("]}");
    let mut request = HttpRequest::new("POST", URL, SENT);
    request
        .headers
        .push((String::from("Authorization"), format!("Token {token}")));
    request.headers.push((
        String::from("Content-Type"),
        String::from("application/json"),
    ));
    request.body = Some(body);
    request.params = Params::new().with(COUNT, notes.highlights.len().to_string());
    request
}

fn notify(env: &Env<'_>, key: &str, out: &mut Vec<Command>) {
    out.push(Command::Notify(env.i18n.tr(key).to_string()));
}

/// Sends the highlights in the notes file at `path`, or asks for the
/// token first.
fn send(env: &Env<'_>, path: &str, out: &mut Vec<Command>) {
    if !env.network {
        return notify(env, "readwise.no-network", out);
    }
    let Some(token) = env.secrets.get(SECRET) else {
        notify(env, "readwise.needs-token", out);
        out.push(Command::OpenSlot(
            slots::KEYBOARD,
            Params::new()
                .with(params::TITLE, env.i18n.tr("readwise.token").to_string())
                .with(params::MODE, "password")
                .with(params::REPLY, TOKEN_ENTERED.as_str())
                .with(params::PATH, path.to_string()),
        ));
        return;
    };
    let Ok(data) = quireink_sdk::read_file(env.storage, path, MAX_FILE) else {
        return notify(env, "readwise.failed", out);
    };
    let notes = parse(&String::from_utf8_lossy(&data));
    if notes.highlights.is_empty() {
        return notify(env, "readwise.nothing", out);
    }
    out.push(Command::Http(request(&notes, token)));
}

/// Readwise answered.
fn sent(env: &Env<'_>, p: &Params, out: &mut Vec<Command>) {
    let status: u16 = p
        .get(http_params::STATUS)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    match status {
        200..=299 => {
            let count: i64 = p.get(COUNT).and_then(|c| c.parse().ok()).unwrap_or(0);
            let text = env
                .i18n
                .plural("readwise.sent", count, &[("count", Arg::Int(count))]);
            out.push(Command::Notify(text));
        }
        401 | 403 => {
            // Asked for anew with the next export.
            out.push(Command::SetSecret(SecretChange::remove(SECRET)));
            notify(env, "readwise.bad-token", out);
        }
        0 => notify(env, "readwise.unreachable", out),
        _ => notify(env, "readwise.failed", out),
    }
}

/// The Readwise plugin.
#[derive(Debug, Default)]
pub struct Readwise;

impl Plugin for Readwise {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Readwise"),
            description: Cow::Borrowed("Sends the highlights of exported notes to Readwise."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.subscribe(
            EXPORTED,
            Box::new(|env: &Env<'_>, p: &Params, out: &mut Vec<Command>| {
                if let Some(path) = p.get(params::PATH) {
                    send(env, path, out);
                }
            }),
        )?;
        r.subscribe(
            TOKEN_ENTERED,
            Box::new(|env: &Env<'_>, p: &Params, out: &mut Vec<Command>| {
                let token = p.get(params::TEXT).unwrap_or_default().trim();
                if token.is_empty() {
                    return;
                }
                out.push(Command::SetSecret(SecretChange::keep(
                    SECRET,
                    "readwise.token",
                    token,
                )));
                // Not kept yet while this runs: sent with it directly.
                let Some(path) = p.get(params::PATH) else {
                    return;
                };
                let Ok(data) = quireink_sdk::read_file(env.storage, path, MAX_FILE) else {
                    return;
                };
                let notes = parse(&String::from_utf8_lossy(&data));
                if !notes.highlights.is_empty() && env.network {
                    out.push(Command::Http(request(&notes, token)));
                }
            }),
        )?;
        r.subscribe(SENT, Box::new(sent))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_i18n::{Catalog, check};

    const NOTES: &str = "# Alice's Adventures\n\nLewis Carroll\n\n## Highlights\n\n\
        > Curiouser and curiouser!\n>\n> 12 %\n\n\
        > Who are you?\n>\n> 40 %\n\nThe caterpillar asks.\nTwice.\n\n\
        ## Bookmarks\n\n- 50 %: Down the rabbit hole\n";

    #[test]
    fn reads_the_readers_markdown() {
        let notes = parse(NOTES);
        assert_eq!(notes.title, "Alice's Adventures");
        assert_eq!(notes.author, "Lewis Carroll");
        assert_eq!(
            notes.highlights,
            [
                Highlight {
                    text: String::from("Curiouser and curiouser!"),
                    percent: 12,
                    note: String::new(),
                },
                Highlight {
                    text: String::from("Who are you?"),
                    percent: 40,
                    note: String::from("The caterpillar asks.\nTwice."),
                },
            ]
        );
        // Without an author or highlights.
        let notes = parse("# Title\n\n## Bookmarks\n\n- 5 %: x\n");
        assert_eq!((notes.title.as_str(), notes.author.as_str()), ("Title", ""));
        assert!(notes.highlights.is_empty());
    }

    #[test]
    fn sends_json_with_the_token() {
        let request = request(&parse(NOTES), "t0k");
        assert_eq!(request.method, "POST");
        assert!(
            request
                .headers
                .contains(&(String::from("Authorization"), String::from("Token t0k")))
        );
        let body = request.body.unwrap();
        assert!(body.starts_with("{\"highlights\":[{\"text\":\"Curiouser and curiouser!\""));
        assert!(body.contains("\"author\":\"Lewis Carroll\""));
        assert!(body.contains("\"note\":\"The caterpillar asks.\\nTwice.\""));
        assert!(body.contains("\"location\":40,\"location_type\":\"order\""));
        assert_eq!(request.params.get(COUNT), Some("2"));
    }

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

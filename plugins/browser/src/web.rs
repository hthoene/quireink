//! Addresses, downloads and pages: what was typed into an address, what
//! an answer is (a page, a book, a font) and where it goes.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::document::is_external;
use quireink_sdk::web::{encode, file_name};
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::{Storage, StorageError};

/// Where searches go: a search engine whose results work without
/// JavaScript.
const SEARCH: &str = "https://html.duckduckgo.com/html/?q=";

/// What an answer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A web page, shown in the reader.
    Page,
    /// A book (EPUB, text), saved in the library.
    Book,
    /// A font (TrueType, OpenType), saved with the fonts.
    Font,
    /// Anything else, saved as a download.
    Other,
}

/// The address to load for what was typed: an address as it is (with
/// `https://` if it has none), else a search for the words.
pub(crate) fn address(input: &str) -> String {
    let text = input.trim();
    if is_external(text) {
        return text.to_string();
    }
    let looks_like_address = !text.contains(char::is_whitespace)
        && text.contains('.')
        && !text.starts_with('.')
        && !text.ends_with('.');
    if looks_like_address {
        format!("https://{text}")
    } else {
        format!("{SEARCH}{}", encode(text))
    }
}

/// The extension of `name`, in lower case.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// What an answer is, by its content type, else its address, else its
/// first bytes (`head`).
pub(crate) fn kind(content_type: Option<&str>, url: &str, head: &[u8]) -> Kind {
    let mime = content_type
        .and_then(|t| t.split(';').next())
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_default();
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => return Kind::Page,
        "application/epub+zip" | "text/plain" => return Kind::Book,
        "font/ttf"
        | "font/otf"
        | "font/sfnt"
        | "font/collection"
        | "application/font-sfnt"
        | "application/x-font-ttf"
        | "application/x-font-otf"
        | "application/vnd.ms-opentype" => {
            return Kind::Font;
        }
        _ => {}
    }
    match extension(&file_name(url)).as_str() {
        "epub" | "txt" => return Kind::Book,
        "ttf" | "otf" | "ttc" => return Kind::Font,
        "html" | "htm" | "xhtml" => return Kind::Page,
        _ => {}
    }
    if head.starts_with(b"OTTO")
        || head.starts_with(&[0, 1, 0, 0])
        || head.starts_with(b"true")
        || head.starts_with(b"ttcf")
    {
        return Kind::Font;
    }
    // An EPUB's first entry is its media type, stored.
    if head.starts_with(b"PK") && head.windows(20).any(|w| w == b"application/epub+zip") {
        return Kind::Book;
    }
    let text = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let start = text.iter().position(|b| !b.is_ascii_whitespace());
    if mime.is_empty() && start.is_some_and(|i| text[i] == b'<') {
        return Kind::Page;
    }
    Kind::Other
}

/// The title of a page in storage, from its `<title>`.
pub(crate) fn title(storage: &dyn Storage, path: &str) -> Option<String> {
    let mut head = alloc::vec![0u8; 16 * 1024];
    let n = storage.read_at(path, 0, &mut head).ok()?;
    let mut title: Option<String> = None;
    let mut done = false;
    Tokenizer::parse(&head[..n], &mut |token| match token {
        Token::Start { name, .. } if !done && local_name(name).eq_ignore_ascii_case("title") => {
            title = Some(String::new());
        }
        Token::End { name } if local_name(name).eq_ignore_ascii_case("title") => done = true,
        Token::Text(text) if !done => {
            if let Some(title) = title.as_mut() {
                title.push_str(&decode(&text));
            }
        }
        _ => {}
    });
    let title = title?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// Writes the page downloaded to `from` to `to`, preceded by its address
/// as `<base href>`, so its links lead where they should.
pub(crate) fn keep_page(
    storage: &dyn Storage,
    from: &str,
    to: &str,
    url: &str,
) -> Result<(), StorageError> {
    let address = url.replace('&', "&amp;").replace('"', "&quot;");
    storage.write(to, format!("<base href=\"{address}\">\n").as_bytes())?;
    let mut chunk = alloc::vec![0u8; 16 * 1024];
    let mut at = 0u64;
    loop {
        let n = storage.read_at(from, at, &mut chunk)?;
        if n == 0 {
            break;
        }
        storage.append(to, &chunk[..n])?;
        at += n as u64;
    }
    storage.remove(from)
}

/// A stable name for the page at `url`, so a page shown again opens
/// where it was left.
pub(crate) fn page_name(url: &str) -> String {
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}.html")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn addresses_and_searches() {
        assert_eq!(address(" de.wikipedia.org "), "https://de.wikipedia.org");
        assert_eq!(address("http://a.org/x"), "http://a.org/x");
        assert_eq!(
            address("Kafka Verwandlung"),
            "https://html.duckduckgo.com/html/?q=Kafka+Verwandlung"
        );
        assert_eq!(
            address("Größe"),
            "https://html.duckduckgo.com/html/?q=Gr%C3%B6%C3%9Fe"
        );
        assert_eq!(
            address("localhost"),
            "https://html.duckduckgo.com/html/?q=localhost"
        );
    }

    #[test]
    fn answers_are_told_apart() {
        assert_eq!(
            kind(Some("text/html; charset=utf-8"), "https://a.org/", b""),
            Kind::Page
        );
        assert_eq!(
            kind(Some("application/epub+zip"), "https://a.org/get?id=3", b""),
            Kind::Book
        );
        assert_eq!(
            kind(Some("application/octet-stream"), "https://a.org/a.ttf", b""),
            Kind::Font
        );
        assert_eq!(kind(None, "https://a.org/x", b"OTTO\0\x0a"), Kind::Font);
        let epub = b"PK\x03\x04\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0mimetypeapplication/epub+zip";
        assert_eq!(
            kind(Some("application/octet-stream"), "https://a.org/dl", epub),
            Kind::Book
        );
        assert_eq!(
            kind(None, "https://a.org/x", b"\n <!DOCTYPE html>"),
            Kind::Page
        );
        assert_eq!(
            kind(Some("application/pdf"), "https://a.org/x.pdf", b"%PDF"),
            Kind::Other
        );
    }

    #[test]
    fn pages_keep_their_address_and_title() {
        let storage = MemoryStorage::new().with(
            "tmp",
            b"<html><head><title> Ein\n Titel </title></head><body>x</body></html>".to_vec(),
        );
        keep_page(&storage, "tmp", "page.html", "https://a.org/?a=1&b=2").unwrap();
        let page = quireink_sdk::read_file(&storage, "page.html", 1024).unwrap();
        assert!(page.starts_with(b"<base href=\"https://a.org/?a=1&amp;b=2\">\n<html>"));
        assert!(!storage.exists("tmp"));
        assert_eq!(title(&storage, "page.html").as_deref(), Some("Ein Titel"));
        assert_eq!(page_name("https://a.org/"), page_name("https://a.org/"));
        assert_ne!(page_name("https://a.org/"), page_name("https://a.org/x"));
    }
}

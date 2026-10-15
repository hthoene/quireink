//! Web addresses and HTTP helpers shared by plugins that fetch things:
//! resolving links, file names for downloads, query encoding and `Basic`
//! authorisation.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::document::is_external;

/// `href` made absolute with `base`, a page's address: as browsers
/// resolve links (`/a`, `../b`, `?q`, `//host/c`).
pub fn join_url(base: &str, href: &str) -> String {
    if is_external(href) {
        return href.to_string();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return href.to_string();
    };
    if let Some(net) = href.strip_prefix("//") {
        return format!("{scheme}://{net}");
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..host_end];
    let after = &rest[host_end..];
    let path = after.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    let joined = if href.starts_with('/') {
        href.to_string()
    } else if href.starts_with('?') {
        format!("{path}{href}")
    } else if href.starts_with('#') {
        let without = after.split('#').next().unwrap_or("");
        format!("{}{href}", if without.is_empty() { "/" } else { without })
    } else {
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{dir}{href}")
    };
    format!("{scheme}://{host}{}", normalize(&joined))
}

/// Resolves `.` and `..` in the path of `target`, keeping its query and
/// fragment.
fn normalize(target: &str) -> String {
    let split = target.find(['?', '#']).unwrap_or(target.len());
    let (path, tail) = target.split_at(split);
    let mut parts: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {
                if last {
                    parts.push("");
                }
            }
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
                if last {
                    parts.push("");
                }
            }
            segment => parts.push(segment),
        }
    }
    let mut out = parts.join("/");
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    out.push_str(tail);
    out
}

/// `text` for a query: letters, digits and `-_.~` as they are, spaces as
/// `+`, everything else percent-encoded.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `text` with `%XX` sequences decoded (invalid UTF-8 replaced).
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let hex = |b: u8| char::from(b).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push(high << 4 | low);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The host of an address, for messages: `de.wikipedia.org`.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    &rest[..end]
}

/// A file name for what `url` points at: its last path part, decoded,
/// without characters file systems refuse; `download` if it has none.
pub fn file_name(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.split(['?', '#']).next().unwrap_or("");
    let last = path.split_once('/').map_or("", |(_, path)| path);
    let last = last.rsplit('/').find(|part| !part.is_empty()).unwrap_or("");
    clean_name(&percent_decode(last), 200).unwrap_or_else(|| String::from("download"))
}

/// `name` as a file name: without characters file systems refuse, not
/// hidden, at most `max` characters; `None` if nothing is left.
pub fn clean_name(name: &str, max: usize) -> Option<String> {
    let name: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c => c,
        })
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end();
    (!name.is_empty()).then(|| name.to_string())
}

/// The `Authorization` header's value for `user` and `password`.
pub fn basic_auth(user: &str, password: &str) -> String {
    format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_resolve_as_in_browsers() {
        let base = "https://example.org/blog/2024/post.html?x=1#top";
        let cases = [
            ("other.html", "https://example.org/blog/2024/other.html"),
            ("../index.html", "https://example.org/blog/index.html"),
            ("../../../../a", "https://example.org/a"),
            ("/about", "https://example.org/about"),
            ("?page=2", "https://example.org/blog/2024/post.html?page=2"),
            ("#c3", "https://example.org/blog/2024/post.html?x=1#c3"),
            ("//cdn.example.net/f.epub", "https://cdn.example.net/f.epub"),
            ("mailto:a@b.c", "mailto:a@b.c"),
            ("./", "https://example.org/blog/2024/"),
        ];
        for (href, expected) in cases {
            assert_eq!(join_url(base, href), expected, "{href}");
        }
        assert_eq!(
            join_url("https://example.org", "a"),
            "https://example.org/a"
        );
    }

    #[test]
    fn names_and_hosts() {
        assert_eq!(
            file_name("https://a.org/books/Die%20Verwandlung.epub?x=1"),
            "Die Verwandlung.epub"
        );
        assert_eq!(file_name("https://a.org/"), "download");
        assert_eq!(file_name("https://a.org"), "download");
        assert_eq!(file_name("https://a.org/f/..%2F.hidden"), "hidden");
        assert_eq!(
            clean_name(" Kafka: Die  Verwandlung? ", 100).as_deref(),
            Some("Kafka Die Verwandlung")
        );
        assert_eq!(clean_name("...", 100), None);
        assert_eq!(host("https://de.wikipedia.org/wiki/X"), "de.wikipedia.org");
        assert_eq!(encode("Größe a"), "Gr%C3%B6%C3%9Fe+a");
    }

    #[test]
    fn credentials() {
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(
            basic_auth("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }
}

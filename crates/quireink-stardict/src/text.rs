//! Turning StarDict definition data into plain text.

use alloc::string::String;
use alloc::vec::Vec;

/// The text of a definition's fields, typed by `types` (the dictionary's
/// `sametypesequence`) or by a type byte before each field.
pub(crate) fn decode(data: &[u8], types: Option<&str>) -> String {
    let mut fields: Vec<(u8, &[u8])> = Vec::new();
    let mut rest = data;
    match types {
        Some(types) => {
            let types = types.as_bytes();
            for (i, &kind) in types.iter().enumerate() {
                let last = i + 1 == types.len();
                let (field, after) = split_field(rest, kind, last);
                fields.push((kind, field));
                rest = after;
            }
        }
        None => {
            while let Some((&kind, after)) = rest.split_first() {
                let (field, after) = split_field(after, kind, false);
                fields.push((kind, field));
                rest = after;
            }
        }
    }
    let mut text = String::new();
    for (kind, field) in fields {
        let field = String::from_utf8_lossy(field);
        let part = match kind {
            b'h' | b'g' | b'x' | b'k' | b'w' | b'r' => to_plain_text(&field),
            b't' => alloc::format!("[{}]", field.trim()),
            b'm' | b'l' | b'y' => field.trim().into(),
            // Binary data (sounds, pictures) and unknown types.
            _ => continue,
        };
        if part.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&part);
    }
    text
}

/// One field: text up to a NUL byte (or the rest, for the last field),
/// binary data with a 4-byte size before it.
fn split_field(data: &[u8], kind: u8, last: bool) -> (&[u8], &[u8]) {
    if kind.is_ascii_uppercase() {
        if last {
            return (data, &[]);
        }
        let Some(size) = data.get(..4) else {
            return (&[], &[]);
        };
        let size = u32::from_be_bytes([size[0], size[1], size[2], size[3]]) as usize;
        let end = (4 + size).min(data.len());
        return (&data[4..end], &data[end..]);
    }
    if last {
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        return (&data[..end], &[]);
    }
    match data.iter().position(|&b| b == 0) {
        Some(end) => (&data[..end], &data[end + 1..]),
        None => (data, &[]),
    }
}

/// Plain text from simple markup (HTML, XDXF, Pango): tags are dropped,
/// block elements start new lines, list items get a bullet, entities are
/// decoded and white space is collapsed.
pub fn to_plain_text(markup: &str) -> String {
    let mut out = String::new();
    let mut chars = markup.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                let mut tag = String::new();
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                    tag.push(c);
                }
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .map(|c| c.to_ascii_lowercase())
                    .collect();
                match name.as_str() {
                    "br" | "p" | "div" | "tr" | "ol" | "ul" | "dd" | "dt" | "blockquote" | "h1"
                    | "h2" | "h3" | "h4" | "h5" | "h6" | "def" | "ex" => out.push('\n'),
                    "li" if !tag.starts_with('/') => out.push_str("\n• "),
                    _ => {}
                }
            }
            '&' => {
                let mut entity = String::new();
                while let Some(&next) = chars.peek() {
                    if next == ';' || entity.len() > 8 {
                        break;
                    }
                    entity.push(next);
                    chars.next();
                }
                if chars.peek() == Some(&';') {
                    chars.next();
                    out.push_str(entity_text(&entity).as_str());
                } else {
                    out.push('&');
                    out.push_str(&entity);
                }
            }
            '\r' | '\n' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    // Collapse spaces, trim lines, drop empty ones.
    let mut text = String::new();
    for line in out.split('\n') {
        let line: Vec<&str> = line.split(' ').filter(|w| !w.is_empty()).collect();
        if line.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line.join(" "));
    }
    text
}

fn entity_text(entity: &str) -> String {
    let c = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "laquo" => '«',
        "raquo" => '»',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "middot" => '·',
        _ => {
            let code = if let Some(hex) = entity.strip_prefix("#x").or(entity.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                entity.strip_prefix('#').and_then(|d| d.parse().ok())
            };
            match code.and_then(char::from_u32) {
                Some(c) => c,
                None => return alloc::format!("&{entity};"),
            }
        }
    };
    String::from(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_plain_text() {
        let html = "<p><b>hus</b> <i>Substantiv</i></p><ol><li>das Haus</li><li>das &laquo;Heim&raquo; &amp; Co</li></ol>";
        assert_eq!(
            to_plain_text(html),
            "hus Substantiv\n• das Haus\n• das «Heim» & Co"
        );
        assert_eq!(to_plain_text("a&#228;b&#xF6;c"), "aäböc");
    }

    #[test]
    fn fields_by_type() {
        assert_eq!(decode(b"tone\0<b>Ton</b>", Some("th")), "[tone]\nTon");
        assert_eq!(
            decode(b"mfirst\0hsecond<br>line\0", None),
            "first\nsecond\nline"
        );
    }
}

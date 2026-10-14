use alloc::borrow::Cow;
use alloc::string::String;

/// Named entities common in e-books (XML's five plus frequent HTML ones).
const NAMED: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{a0}'),
    ("shy", '\u{ad}'),
    ("ensp", '\u{2002}'),
    ("emsp", '\u{2003}'),
    ("thinsp", '\u{2009}'),
    ("zwnj", '\u{200c}'),
    ("zwj", '\u{200d}'),
    ("ndash", '–'),
    ("mdash", '—'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("sbquo", '‚'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("bdquo", '„'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("lsaquo", '‹'),
    ("rsaquo", '›'),
    ("hellip", '…'),
    ("bull", '•'),
    ("middot", '·'),
    ("dagger", '†'),
    ("Dagger", '‡'),
    ("prime", '′'),
    ("Prime", '″'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("deg", '°'),
    ("times", '×'),
    ("divide", '÷'),
    ("plusmn", '±'),
    ("frac12", '½'),
    ("frac14", '¼'),
    ("frac34", '¾'),
    ("sect", '§'),
    ("para", '¶'),
    ("euro", '€'),
    ("pound", '£'),
    ("yen", '¥'),
    ("cent", '¢'),
    ("iexcl", '¡'),
    ("iquest", '¿'),
    ("szlig", 'ß'),
    ("auml", 'ä'),
    ("ouml", 'ö'),
    ("uuml", 'ü'),
    ("Auml", 'Ä'),
    ("Ouml", 'Ö'),
    ("Uuml", 'Ü'),
    ("aacute", 'á'),
    ("eacute", 'é'),
    ("iacute", 'í'),
    ("oacute", 'ó'),
    ("uacute", 'ú'),
    ("agrave", 'à'),
    ("egrave", 'è'),
    ("igrave", 'ì'),
    ("ograve", 'ò'),
    ("ugrave", 'ù'),
    ("acirc", 'â'),
    ("ecirc", 'ê'),
    ("icirc", 'î'),
    ("ocirc", 'ô'),
    ("ucirc", 'û'),
    ("ccedil", 'ç'),
    ("ntilde", 'ñ'),
    ("aring", 'å'),
    ("aelig", 'æ'),
    ("oslash", 'ø'),
    ("Eacute", 'É'),
    ("Ccedil", 'Ç'),
];

/// Decodes character and entity references; unknown or malformed ones are
/// kept as written.
pub fn decode(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail[1..]
            .find(';')
            .filter(|&end| end <= 32)
            .and_then(|end| resolve(&tail[1..1 + end]).map(|c| (c, end + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn resolve(entity: &str) -> Option<char> {
    if let Some(number) = entity.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code).filter(|&c| c != '\0');
    }
    NAMED
        .iter()
        .find(|(name, _)| *name == entity)
        .map(|(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_named_and_numeric_references() {
        assert_eq!(decode("a &amp; b"), "a & b");
        assert_eq!(decode("&#8220;Hi&#x201D; &mdash; &auml;"), "“Hi” — ä");
        assert_eq!(decode("&unknown; & &#xZZ; &;"), "&unknown; & &#xZZ; &;");
        assert!(matches!(decode("plain"), Cow::Borrowed(_)));
    }
}

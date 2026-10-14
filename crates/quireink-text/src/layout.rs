//! Line breaking.

use alloc::vec::Vec;
use core::ops::Range;

/// Breaks `text` into lines no wider than `max_width`, using `measure` to
/// get the width of a slice. Breaks at spaces and after hyphens; explicit
/// newlines always break; words longer than a line are split by character.
///
/// Returned ranges exclude the whitespace at which a line was broken.
pub fn wrap(text: &str, max_width: f32, measure: impl Fn(&str) -> f32) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for paragraph in text.split('\n') {
        wrap_paragraph(paragraph, offset, max_width, &measure, &mut lines);
        offset += paragraph.len() + 1;
    }
    lines
}

fn wrap_paragraph(
    text: &str,
    base: usize,
    max_width: f32,
    measure: &impl Fn(&str) -> f32,
    lines: &mut Vec<Range<usize>>,
) {
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut start = 0;
    if text.is_empty() {
        lines.push(base..base);
        return;
    }
    while start < text.len() {
        // Skip spaces at the start of a continuation line.
        if start > 0 {
            start += text[start..].len() - text[start..].trim_start_matches(' ').len();
            if start >= text.len() {
                break;
            }
        }
        let rest = &text[start..];
        if measure(rest) <= max_width {
            lines.push(base + start..base + text.len());
            break;
        }
        // Last break opportunity that still fits.
        let mut best = None;
        for (i, c) in rest.char_indices() {
            let end = match c {
                ' ' => i,
                '-' | '\u{2010}' | '\u{2013}' | '\u{2014}' => i + c.len_utf8(),
                _ => continue,
            };
            if end == 0 {
                continue;
            }
            if measure(&rest[..end]) <= max_width {
                best = Some(end);
            } else {
                break;
            }
        }
        let end = best.unwrap_or_else(|| split_word(rest, max_width, measure));
        lines.push(base + start..base + start + rest[..end].trim_end_matches(' ').len());
        start += end;
    }
}

/// The longest prefix of `text` that fits, at least one character.
fn split_word(text: &str, max_width: f32, measure: &impl Fn(&str) -> f32) -> usize {
    let mut end = text.chars().next().map_or(text.len(), char::len_utf8);
    for (i, c) in text.char_indices().skip(1) {
        if measure(&text[..i + c.len_utf8()]) > max_width {
            break;
        }
        end = i + c.len_utf8();
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Every character is one unit wide.
    fn lines(text: &str, width: f32) -> Vec<&str> {
        wrap(text, width, |s| s.chars().count() as f32)
            .into_iter()
            .map(|r| &text[r])
            .collect()
    }

    #[test]
    fn breaks_at_spaces() {
        assert_eq!(
            lines("the quick brown fox", 10.0),
            vec!["the quick", "brown fox"]
        );
    }

    #[test]
    fn breaks_after_hyphens_and_at_newlines() {
        assert_eq!(
            lines("well-known\nfact", 6.0),
            vec!["well-", "known", "fact"]
        );
        assert_eq!(lines("a\n\nb", 6.0), vec!["a", "", "b"]);
    }

    #[test]
    fn splits_overlong_words() {
        assert_eq!(lines("abcdefgh", 3.0), vec!["abc", "def", "gh"]);
    }

    #[test]
    fn handles_multibyte_text() {
        assert_eq!(
            lines("Größe über Äpfel", 6.0),
            vec!["Größe", "über", "Äpfel"]
        );
    }
}

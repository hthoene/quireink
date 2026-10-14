//! Line breaking.

use alloc::vec::Vec;
use core::ops::Range;

/// A line produced by [`wrap_lines`].
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Byte range of the line's text, without the whitespace it broke at.
    pub range: Range<usize>,
    /// Width of the text in `range`.
    pub width: f32,
    /// Number of spaces inside `range` (for justification).
    pub spaces: usize,
    /// Whether the line ends its paragraph (or a forced break).
    pub last: bool,
}

/// Greedy line breaking in a single pass over `text`.
///
/// Breaks at spaces and after hyphens and dashes; explicit newlines always
/// break; words longer than a line are split by character. `width(n)` is the
/// available width of line `n` (allowing a first-line indent), `advance(c)`
/// the width of a character.
pub fn wrap_lines(
    text: &str,
    width: impl Fn(usize) -> f32,
    advance: impl Fn(char) -> f32,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut line_start = 0;
    let mut line_width = 0.0f32;
    let mut spaces = 0usize;
    // Last break opportunity: (end of line text, start of next line, width, spaces).
    let mut last_break: Option<(usize, usize, f32, usize)> = None;
    let mut chars = text.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            push(&mut lines, text, line_start..i, line_width, spaces, true);
            line_start = i + 1;
            (line_width, spaces, last_break) = (0.0, 0, None);
            continue;
        }
        if c == ' ' && i == line_start {
            // Spaces at the start of a continuation line are dropped.
            line_start = i + 1;
            continue;
        }
        let w = advance(c);
        let available = width(lines.len());
        if c != ' ' && line_width + w > available && i > line_start {
            // Break at the last opportunity, or inside the word if none.
            let (end, next, end_width, end_spaces) =
                last_break.unwrap_or((i, i, line_width, spaces));
            push(
                &mut lines,
                text,
                line_start..end,
                end_width,
                end_spaces,
                false,
            );
            line_start = next;
            // Re-measure what moved to the new line.
            line_width = text[next..i].chars().map(&advance).sum();
            spaces = text[next..i].matches(' ').count();
            last_break = None;
        }
        line_width += w;
        match c {
            ' ' => {
                let trimmed = text[line_start..i].trim_end_matches(' ');
                let end = line_start + trimmed.len();
                let trailing = text[end..i].chars().count();
                let end_width = line_width - w - trailing as f32 * w;
                let end_spaces = spaces.saturating_sub(trailing);
                last_break = Some((end, i + 1, end_width, end_spaces));
                spaces += 1;
            }
            '-' | '\u{2010}' | '\u{2013}' | '\u{2014}' => {
                let end = i + c.len_utf8();
                if chars.peek().is_some_and(|(_, n)| *n != ' ') {
                    last_break = Some((end, end, line_width, spaces));
                }
            }
            _ => {}
        }
    }
    if line_start < text.len() || lines.is_empty() || text.ends_with('\n') {
        push(
            &mut lines,
            text,
            line_start..text.len(),
            line_width,
            spaces,
            true,
        );
    }
    lines
}

fn push(
    lines: &mut Vec<Line>,
    text: &str,
    range: Range<usize>,
    width: f32,
    spaces: usize,
    last: bool,
) {
    let trimmed = text[range.clone()].trim_end_matches(' ');
    let removed = range.len() - trimmed.len();
    lines.push(Line {
        range: range.start..range.start + trimmed.len(),
        width,
        spaces: spaces.saturating_sub(removed),
        last,
    });
}

/// Breaks `text` into lines no wider than `max_width`, using `measure` to
/// get the width of a slice. Convenience wrapper around [`wrap_lines`] for
/// short UI texts.
pub fn wrap(text: &str, max_width: f32, measure: impl Fn(&str) -> f32) -> Vec<Range<usize>> {
    wrap_lines(
        text,
        |_| max_width,
        |c| measure(c.encode_utf8(&mut [0u8; 4])),
    )
    .into_iter()
    .map(|line| line.range)
    .collect()
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
        assert_eq!(lines("a  b", 1.0), vec!["a", "b"]);
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

    #[test]
    fn reports_widths_spaces_and_first_line_indent() {
        let text = "aa bb cc dd";
        let result = wrap_lines(text, |n| if n == 0 { 6.0 } else { 8.0 }, |_| 1.0);
        let got: Vec<(&str, f32, usize, bool)> = result
            .iter()
            .map(|l| (&text[l.range.clone()], l.width, l.spaces, l.last))
            .collect();
        assert_eq!(got, vec![("aa bb", 5.0, 1, false), ("cc dd", 5.0, 1, true)]);
    }

    #[test]
    fn empty_text_is_one_empty_line() {
        assert_eq!(lines("", 5.0), vec![""]);
    }
}

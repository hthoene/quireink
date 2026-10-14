//! Line breaking.

use alloc::vec::Vec;
use core::ops::Range;

/// A line produced by [`wrap_lines`].
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Byte range of the line's text, without the whitespace it broke at.
    pub range: Range<usize>,
    /// Width of the text in `range`, plus the hyphen if `hyphen` is set.
    pub width: f32,
    /// Number of spaces inside `range` (for justification).
    pub spaces: usize,
    /// Whether the line ends its paragraph (or a forced break).
    pub last: bool,
    /// Whether the line ends inside a hyphenated word: draw a hyphen
    /// after the text.
    pub hyphen: bool,
}

/// Finds hyphenation points: calls the second argument with the byte
/// offset of every position where the word (letters only) may be broken.
pub type Hyphenate<'a> = &'a dyn Fn(&str, &mut dyn FnMut(usize));

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
    wrap_lines_hyphenated(text, width, advance, None)
}

/// Like [`wrap_lines`], but a word that does not fit at the end of a line
/// is hyphenated where `hyphenate` allows, as far as it fits.
///
/// Lines only depend on the text from their start, so laying out from the
/// start of any line gives the same lines as laying out the whole text.
pub fn wrap_lines_hyphenated(
    text: &str,
    width: impl Fn(usize) -> f32,
    advance: impl Fn(char) -> f32,
    hyphenate: Option<Hyphenate<'_>>,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut line_start = 0;
    let mut line_width = 0.0f32;
    let mut spaces = 0usize;
    // Where the word being read began.
    let mut word_start = 0;
    // Last break opportunity: (end of line text, start of next line, width, spaces).
    let mut last_break: Option<(usize, usize, f32, usize)> = None;
    let mut chars = text.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            push(&mut lines, text, line_start..i, line_width, spaces, true);
            line_start = i + 1;
            word_start = line_start;
            (line_width, spaces, last_break) = (0.0, 0, None);
            continue;
        }
        if c == ' ' && i == line_start {
            // Spaces at the start of a continuation line are dropped.
            line_start = i + 1;
            word_start = line_start;
            continue;
        }
        let w = advance(c);
        let available = width(lines.len());
        if c != ' ' && line_width + w > available && i > line_start {
            let measure = |range: Range<usize>| -> f32 { text[range].chars().map(&advance).sum() };
            let before = line_width - measure(word_start..i);
            let hyphenated = hyphenate.and_then(|hyphenate| {
                hyphen_break(
                    text, word_start, line_start, before, available, &measure, &advance, hyphenate,
                )
            });
            if let Some((cut, cut_width)) = hyphenated {
                lines.push(Line {
                    range: line_start..cut,
                    width: cut_width,
                    spaces,
                    last: false,
                    hyphen: true,
                });
                line_start = cut;
                word_start = cut;
                line_width = measure(cut..i);
                spaces = 0;
            } else {
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
                word_start = word_start.max(next);
                // Re-measure what moved to the new line.
                line_width = measure(next..i);
                spaces = text[next..i].matches(' ').count();
            }
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
                word_start = i + 1;
            }
            '-' | '\u{2010}' | '\u{2013}' | '\u{2014}' => {
                let end = i + c.len_utf8();
                if chars.peek().is_some_and(|(_, n)| *n != ' ') {
                    last_break = Some((end, end, line_width, spaces));
                    word_start = end;
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

/// The last hyphenation point of the word at `word_start` that fits on the
/// line: `(byte offset, line width with the hyphen)`. Only words made of
/// letters are hyphenated; punctuation around them stays attached.
#[allow(clippy::too_many_arguments)]
fn hyphen_break(
    text: &str,
    word_start: usize,
    line_start: usize,
    before: f32,
    available: f32,
    measure: &dyn Fn(Range<usize>) -> f32,
    advance: &dyn Fn(char) -> f32,
    hyphenate: Hyphenate<'_>,
) -> Option<(usize, f32)> {
    let end = text[word_start..]
        .find([' ', '\n'])
        .map_or(text.len(), |k| word_start + k);
    let word = &text[word_start..end];
    let first = word.find(char::is_alphabetic)?;
    let (last, c) = word.char_indices().rev().find(|(_, c)| c.is_alphabetic())?;
    let core = &word[first..last + c.len_utf8()];
    if !core.chars().all(char::is_alphabetic) {
        return None;
    }
    let hyphen = advance('-');
    let mut best = None;
    hyphenate(core, &mut |at| {
        let cut = word_start + first + at;
        if cut <= line_start {
            return;
        }
        let width = before + measure(word_start..cut) + hyphen;
        if width <= available {
            best = Some((cut, width));
        }
    });
    best
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
        hyphen: false,
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
    fn hyphenates_words_that_do_not_fit() {
        // Every word may be broken after its third and sixth letter.
        let hyphenate = |word: &str, out: &mut dyn FnMut(usize)| {
            for at in [3, 6] {
                if at < word.len() {
                    out(at);
                }
            }
        };
        let text = "ab abcdefghi, xy";
        let result = wrap_lines_hyphenated(text, |_| 9.0, |_| 1.0, Some(&hyphenate));
        let got: Vec<(&str, f32, bool)> = result
            .iter()
            .map(|l| (&text[l.range.clone()], l.width, l.hyphen))
            .collect();
        // "ab abc-" fits 9 units with the hyphen, "ab abcdef-" would not.
        assert_eq!(
            got,
            vec![
                ("ab abc", 7.0, true),
                ("defghi,", 7.0, false),
                ("xy", 2.0, false)
            ]
        );
        // Without hyphenation the word moves to the next line.
        assert_eq!(lines(text, 9.0), vec!["ab", "abcdefghi", ", xy"]);
    }

    #[test]
    fn hyphenation_is_stable_from_any_line_start() {
        let hyphenate = |word: &str, out: &mut dyn FnMut(usize)| {
            for at in (2..word.len().saturating_sub(1)).step_by(2) {
                out(at);
            }
        };
        let text = "Lorem ipsum dolor sitametconsectetur adipiscing elit sed doeiusmod";
        let all = wrap_lines_hyphenated(text, |_| 12.0, |_| 1.0, Some(&hyphenate));
        for line in &all[1..] {
            let from = line.range.start;
            let rest = wrap_lines_hyphenated(&text[from..], |_| 12.0, |_| 1.0, Some(&hyphenate));
            assert_eq!(
                text[from + rest[0].range.start..from + rest[0].range.end],
                text[line.range.clone()]
            );
        }
    }

    #[test]
    fn empty_text_is_one_empty_line() {
        assert_eq!(lines("", 5.0), vec![""]);
    }
}

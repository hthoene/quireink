//! Reading aloud, page by page, where the device can speak.

use alloc::string::String;

/// The text of a page's lines as it is spoken: words that hyphenation
/// split at a line end are joined again.
pub(crate) fn spoken_text<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut text = String::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut before = text.chars().rev();
        let split = before.next() == Some('-')
            && before.next().is_some_and(char::is_alphabetic)
            && line.chars().next().is_some_and(char::is_lowercase);
        if split {
            text.pop();
        } else if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(line);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::spoken_text;

    #[test]
    fn hyphenated_words_are_joined_and_lines_spaced() {
        let lines = [
            "The quick brown fox jum-",
            "ped over the well-",
            "Known dog.",
        ];
        assert_eq!(
            spoken_text(lines.into_iter()),
            "The quick brown fox jumped over the well- Known dog."
        );
        assert_eq!(spoken_text(["", " a ", "b"].into_iter()), "a b");
    }
}

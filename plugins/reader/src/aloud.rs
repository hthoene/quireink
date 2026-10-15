//! Reading aloud, sentence by sentence, where the device can speak.

use alloc::string::String;

use quireink_sdk::Env;

use crate::settings::ALOUD_TIMER;

/// The most words spoken at once, for sentences without an end.
const MAX_WORDS: usize = 80;

/// Choices of [`ALOUD_TIMER`]: minutes, `off` for none.
pub(crate) const TIMER_CHOICES: [&str; 5] = [
    "reader.aloud-timer.off",
    "reader.aloud-timer.15",
    "reader.aloud-timer.30",
    "reader.aloud-timer.60",
    "reader.aloud-timer.90",
];

/// Minutes until reading aloud stops by itself, if a timer is set.
pub(crate) fn timer_minutes(env: &Env<'_>) -> Option<u64> {
    let choice = env.settings.text(ALOUD_TIMER.as_str());
    choice
        .rsplit('.')
        .next()
        .and_then(|m| m.parse().ok())
        .filter(|&m| m > 0)
}

/// Whether `word` ends a sentence (closing quotes and brackets after
/// its punctuation are part of it).
pub(crate) fn ends_sentence(word: &str) -> bool {
    word.trim_end_matches(['"', '\'', '”', '’', '»', '«', '›', '‹', ')', ']', '“'])
        .ends_with(['.', '!', '?', '…'])
}

/// The end (exclusive) of the sentence that starts at word `from`.
pub(crate) fn sentence_end(words: &[&str], from: usize) -> usize {
    let last = words.len().min(from + MAX_WORDS);
    (from..last)
        .find(|&i| ends_sentence(words[i]))
        .map_or(last, |i| i + 1)
}

/// The text of words as it is spoken, each with whether it starts a line:
/// words that hyphenation split at a line end are joined again.
pub(crate) fn spoken<'a>(words: impl Iterator<Item = (&'a str, bool)>) -> String {
    let mut text = String::new();
    for (word, new_line) in words {
        let mut before = text.chars().rev();
        let split = new_line
            && before.next() == Some('-')
            && before.next().is_some_and(char::is_alphabetic)
            && word.chars().next().is_some_and(char::is_lowercase);
        if split {
            text.pop();
        } else if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(word);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_end_at_their_punctuation() {
        let words = ["“Oh", "dear!”", "she", "said.", "And", "then"];
        assert_eq!(sentence_end(&words, 0), 2);
        assert_eq!(sentence_end(&words, 2), 4);
        // Without an end: up to the last word.
        assert_eq!(sentence_end(&words, 4), 6);
        assert!(ends_sentence("(really?)"));
        assert!(!ends_sentence("well,"));
    }

    #[test]
    fn hyphenated_words_are_joined_at_line_ends() {
        let words = [
            ("The", true),
            ("fox", false),
            ("jum-", false),
            ("ped", true),
            ("over", false),
            ("well-", false),
            ("Known", true),
            ("self-", false),
            ("made", false),
        ];
        assert_eq!(
            spoken(words.into_iter()),
            "The fox jumped over well- Known self- made"
        );
    }
}

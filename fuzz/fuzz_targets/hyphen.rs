//! Any bytes as a pattern table, and any text as a word to hyphenate.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quireink_hyphen::Patterns;

fuzz_target!(|data: &[u8]| {
    let split = data.len() / 2;
    let (table, word) = data.split_at(split);
    if let (Ok(patterns), Ok(word)) = (Patterns::new(table), core::str::from_utf8(word)) {
        patterns.hyphenate(word, &mut |at| assert!(word.is_char_boundary(at)));
    }
});

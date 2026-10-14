//! Any bytes through the XML tokenizer, in chunks of a size the input
//! chooses.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quireink_sdk::xml::{Token, Tokenizer, decode};

fuzz_target!(|data: &[u8]| {
    let Some((&size, data)) = data.split_first() else {
        return;
    };
    let mut tokenizer = Tokenizer::new();
    let mut emit = |token: Token<'_>| {
        if let Token::Text(text) = token {
            let _ = decode(&text);
        }
    };
    for chunk in data.chunks(usize::from(size).max(1)) {
        tokenizer.feed(chunk, &mut emit);
    }
    tokenizer.finish(&mut emit);
});

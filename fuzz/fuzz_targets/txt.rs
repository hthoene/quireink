//! Any bytes as a plain text book, read block by block.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quireink_plugin_format_txt::TxtDocument;
use quireink_sdk::MemoryStorage;
use quireink_sdk::document::{Document, Locator};

fuzz_target!(|data: &[u8]| {
    let storage = MemoryStorage::new().with("book.txt", data.to_vec());
    let Ok(doc) = TxtDocument::open(&storage, "book.txt") else {
        return;
    };
    let mut at = Locator::new(0, 0);
    while let Ok(Some(block)) = doc.block_at(&storage, at) {
        assert!(block.end > at.offset);
        at = Locator::new(0, block.end);
    }
});

//! Covers: found in the package, decoded once into a small gray image.

// Tests panic on purpose when something is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use quireink_plugin_format_epub::EpubDocument;
use quireink_sdk::MemoryStorage;
use quireink_sdk::document::Document;
use quireink_sdk::image::GrayFile;

#[test]
fn covers_are_decoded_small_and_cached() {
    for (name, book) in [
        (
            "alice.epub",
            &include_bytes!("../../../samples/library/books/alice-in-wonderland.epub")[..],
        ),
        (
            "kafka.epub",
            &include_bytes!("../../../samples/library/books/die-verwandlung.epub")[..],
        ),
    ] {
        let storage = MemoryStorage::new().with(name, book);
        let document = EpubDocument::open(&storage, name).unwrap();
        let cover = document.cover(&storage).unwrap().expect(name);
        assert!(cover.width.max(cover.height) <= 300, "{name}: {cover:?}");
        assert!(cover.width > 50 && cover.height > 50, "{name}: {cover:?}");
        let image = GrayFile::open(&storage, &cover.path).unwrap();
        assert_eq!((image.width, image.height), (cover.width, cover.height));
        // Cached: opening the book again finds the same file.
        let again = EpubDocument::open(&storage, name).unwrap();
        assert_eq!(again.cover(&storage).unwrap(), Some(cover));
    }
}

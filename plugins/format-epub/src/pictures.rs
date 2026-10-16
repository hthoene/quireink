//! Decoded pictures in the caches: gray (`.gray`) for e-ink, colour
//! (`.rgb`) for colour screens (ADR 0015). Chapters keep a picture's
//! cache name without its kind; it is decoded in the kind the book was
//! opened with, and read in that kind, else in the other one there is.

use alloc::format;
use alloc::string::String;

use quireink_sdk::Storage;
use quireink_sdk::document::ImageRef;
use quireink_sdk::image::ImageFile;

/// The cache file of picture `base` in colour or gray.
pub(crate) fn file(base: &str, color: bool) -> String {
    format!("{base}.{}", if color { "rgb" } else { "gray" })
}

/// The decoded picture `base`, the kind wanted first, else the other.
pub(crate) fn open(storage: &dyn Storage, base: &str, color: bool) -> Option<ImageRef> {
    [color, !color].into_iter().find_map(|color| {
        ImageFile::open(storage, &file(base, color)).map(|image| ImageRef {
            path: image.path,
            width: image.width,
            height: image.height,
        })
    })
}

/// Whether picture `base` is decoded in the kind wanted.
pub(crate) fn has(storage: &dyn Storage, base: &str, color: bool) -> bool {
    ImageFile::open(storage, &file(base, color)).is_some()
}

//! The covers of recent books, from the library service
//! ([`quireink_sdk::library`]). Looking a cover up opens the book, so the
//! home screen does it a book at a time while it shows, and its widgets
//! only read what the library knows.

use quireink_sdk::document::ImageRef;
use quireink_sdk::{Env, Storage, library};

/// The cover of `path`: `None` if not looked up yet, `Some(None)` if it
/// has none.
pub(crate) fn get(storage: &dyn Storage, path: &str) -> Option<Option<ImageRef>> {
    let book = library::book(storage, path).filter(|b| b.indexed)?;
    Some(book.cover.filter(|i| storage.exists(&i.path)))
}

/// Looks up the cover of `path` (opening the book), for the library to
/// keep.
pub(crate) fn look_up(env: &Env<'_>, path: &str) -> Option<ImageRef> {
    library::index(env, path).cover
}

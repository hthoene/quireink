//! The recently read books, from the library service
//! ([`quireink_sdk::library`]), which follows the standard reading
//! events, so any reader plugin works.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::{Storage, library};

/// Books shown at most.
const LIMIT: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recent {
    pub path: String,
    pub title: String,
    /// Percent, 0 to 100.
    pub progress: f32,
}

/// The books read last, most recent first.
pub(crate) fn load(storage: &dyn Storage) -> Vec<Recent> {
    library::opened_books(storage)
        .into_iter()
        .take(LIMIT)
        .map(|path| {
            let book = library::book(storage, &path).unwrap_or_default();
            Recent {
                title: book.title_or(&path).to_string(),
                progress: f32::from(book.progress.unwrap_or(0)),
                path,
            }
        })
        .collect()
}

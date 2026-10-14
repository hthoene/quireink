//! What the library knows about books beyond their files: title, author,
//! cover and reading progress (one small file per book, filled in when a
//! book is first shown), and the order in which books were last opened.
//!
//! Nothing here grows with the size of the library: the list of opened
//! books is capped, and book details are read only for visible rows.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::params;
use quireink_sdk::config;
use quireink_sdk::document::ImageRef;
use quireink_sdk::{Env, Params, Storage};

use crate::PLUGIN;

/// Books remembered for "recently read" order.
const MAX_OPENED: usize = 100;

/// Details of a book, cached in storage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Book {
    /// File size when the details were read; another size means another
    /// file.
    pub size: u64,
    /// Title, author and cover were read from the book.
    pub indexed: bool,
    pub title: Option<String>,
    pub author: Option<String>,
    pub cover: Option<ImageRef>,
    /// Reading progress in percent, once opened.
    pub progress: Option<u8>,
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn book_file(path: &str) -> String {
    format!(
        "{}/books/{:016x}.ini",
        Env::plugin_dir(PLUGIN.as_str()),
        fnv(path)
    )
}

fn opened_file() -> String {
    format!("{}/opened.txt", Env::plugin_dir(PLUGIN.as_str()))
}

/// The cached details of the book at `path`, if they belong to this file.
pub(crate) fn load(storage: &dyn Storage, path: &str) -> Option<Book> {
    let size = storage.size(path).ok()?;
    let data = quireink_sdk::read_file(storage, &book_file(path), 4096).ok()?;
    let source = core::str::from_utf8(&data).ok()?;
    let mut book = Book::default();
    let (mut cover_path, mut cover_width, mut cover_height) = (None, None, None);
    for entry in config::parse(source).filter_map(Result::ok) {
        let text = || entry.value.as_str().into_owned();
        match entry.key {
            "size" => book.size = entry.value.raw().parse().unwrap_or_default(),
            "indexed" => book.indexed = entry.value.as_bool().unwrap_or_default(),
            "title" => book.title = Some(text()),
            "author" => book.author = Some(text()),
            "cover" => cover_path = Some(text()),
            "cover-width" => cover_width = entry.value.as_u32(),
            "cover-height" => cover_height = entry.value.as_u32(),
            "progress" => book.progress = entry.value.as_u32().map(|p| p.min(100) as u8),
            _ => {}
        }
    }
    if let (Some(path), Some(width), Some(height)) = (cover_path, cover_width, cover_height) {
        book.cover = Some(ImageRef {
            path,
            width,
            height,
        });
    }
    (book.size == size).then_some(book)
}

fn save(storage: &dyn Storage, path: &str, book: &Book) {
    let mut ini = String::new();
    let mut set = |key: &str, value: &str| ini = config::set(&ini, "book", key, value);
    set("size", &book.size.to_string());
    set("indexed", if book.indexed { "on" } else { "off" });
    if let Some(title) = &book.title {
        set("title", title);
    }
    if let Some(author) = &book.author {
        set("author", author);
    }
    if let Some(cover) = &book.cover {
        set("cover", &cover.path);
        set("cover-width", &cover.width.to_string());
        set("cover-height", &cover.height.to_string());
    }
    if let Some(progress) = book.progress {
        set("progress", &progress.to_string());
    }
    let _ = storage.write(&book_file(path), ini.as_bytes());
}

/// Reads title, author and cover from the book itself and caches them.
/// Opening a book the first time may take a moment (e.g. an EPUB's
/// package is read), so the library does this for one book at a time.
pub(crate) fn index(env: &Env<'_>, path: &str) -> Book {
    let storage = env.storage;
    let size = storage.size(path).unwrap_or_default();
    let mut book = load(storage, path).unwrap_or_default();
    book.size = size;
    book.indexed = true;
    if let Ok(document) = env.formats.open(storage, path) {
        let metadata = document.metadata();
        book.title = metadata.title.clone();
        book.author = metadata.authors.first().cloned();
        book.cover = document.cover(storage).ok().flatten();
    }
    save(storage, path, &book);
    book
}

/// Records a reading event (`core.book-opened`, `core.page-turned`):
/// the book's progress, and for an opened book its place in the
/// "recently read" order.
pub(crate) fn record(env: &Env<'_>, params: &Params, opened: bool) {
    let Some(path) = params.get(params::PATH) else {
        return;
    };
    let storage = env.storage;
    if opened {
        let mut list = load_opened(storage);
        list.retain(|p| p != path);
        list.insert(0, path.to_string());
        list.truncate(MAX_OPENED);
        let _ = storage.write(&opened_file(), list.join("\n").as_bytes());
    }
    let progress = params
        .get(params::PROGRESS)
        .and_then(|p| p.parse::<f32>().ok())
        .map(|p| (p + 0.5).clamp(0.0, 100.0) as u8);
    let Some(progress) = progress else {
        return;
    };
    let mut book = load(storage, path).unwrap_or_else(|| Book {
        size: storage.size(path).unwrap_or_default(),
        ..Book::default()
    });
    // Written only when the shown percentage changes.
    if book.progress != Some(progress) {
        book.progress = Some(progress);
        save(storage, path, &book);
    }
}

/// Paths of the books opened last, most recent first.
pub(crate) fn load_opened(storage: &dyn Storage) -> Vec<String> {
    let max = MAX_OPENED * 512;
    let Ok(data) = quireink_sdk::read_file(storage, &opened_file(), max) else {
        return Vec::new();
    };
    core::str::from_utf8(&data)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn details_belong_to_one_file_version() {
        let storage = MemoryStorage::new().with("books/a.txt", "hello");
        let book = Book {
            size: 5,
            indexed: true,
            title: Some("A = \"title\"".into()),
            author: Some("Ann".into()),
            cover: Some(ImageRef {
                path: "x.gray".into(),
                width: 30,
                height: 40,
            }),
            progress: Some(12),
        };
        save(&storage, "books/a.txt", &book);
        assert_eq!(load(&storage, "books/a.txt"), Some(book));
        // Replaced by a file of another size: forgotten.
        let storage = storage.with("books/a.txt", "hello!");
        assert_eq!(load(&storage, "books/a.txt"), None);
    }
}

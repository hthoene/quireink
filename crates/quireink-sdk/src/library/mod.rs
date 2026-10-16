//! The library as a service: what is known about books beyond their
//! files, for every plugin that shows or uses them (the library, the home
//! screen, the sleep screen, syncs). Data and the screens that show it
//! are apart (PLANNING §6.3): any plugin may offer another view of the
//! same books.
//!
//! - A book's details: title, author, series, tags and cover, read from
//!   the book once ([`index`]), its reading progress, and fields that
//!   plugins add (e.g. a rating or where a book came from), in
//!   `.quireink/library/books/<hash>.ini`.
//! - The books opened last, most recent first (`opened.txt`), and the
//!   favourites (`favourites.txt`).
//! - The catalog of all books under the library folder ([`catalog`]),
//!   and listings of a folder's books ([`listing`]).
//!
//! The kernel keeps them up to date from the standard reading events and
//! moves them with a book ([`record`], [`moved`]). Nothing here grows
//! with the size of the library: the lists are capped, and details are
//! read a book at a time.

pub mod catalog;
pub mod listing;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::builtin::params;
use crate::config;
use crate::document::ImageRef;
use crate::{Env, Params, Storage};

/// Where the library keeps what it knows.
pub const DIR: &str = ".quireink/library";
/// Books remembered for "recently read" order.
const MAX_OPENED: usize = 100;
/// Favourites kept at most.
const MAX_FAVOURITES: usize = 500;
/// What the details hold; books read with another version are read
/// again (2 added the series, 3 the tags).
const DETAILS: u32 = 3;

/// What the library knows of a book.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Book {
    /// File size when the details were read; another size means another
    /// file.
    pub size: u64,
    /// Title, author and cover were read from the book.
    pub indexed: bool,
    pub title: Option<String>,
    pub author: Option<String>,
    /// The series and the book's number in it, if it names one.
    pub series: Option<String>,
    pub series_index: Option<String>,
    /// Its tags (subjects), if it names any.
    pub tags: Vec<String>,
    pub cover: Option<ImageRef>,
    /// Reading progress in percent, once opened.
    pub progress: Option<u8>,
    /// Fields plugins keep with the book: id (e.g. `myplugin.rating`)
    /// and value.
    pub fields: Vec<(String, String)>,
}

impl Book {
    /// The value of field `id`, if set.
    pub fn field(&self, id: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.as_str())
    }

    /// The title, else the file's name without its extension.
    pub fn title_or<'a>(&'a self, path: &'a str) -> &'a str {
        match &self.title {
            Some(title) if !title.is_empty() => title,
            _ => {
                let name = crate::files::name(path);
                name.rsplit_once('.').map_or(name, |(stem, _)| stem)
            }
        }
    }
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn book_file(path: &str) -> String {
    format!("{DIR}/books/{:016x}.ini", fnv(path))
}

fn opened_file() -> String {
    format!("{DIR}/opened.txt")
}

fn favourites_file() -> String {
    format!("{DIR}/favourites.txt")
}

/// The lines of a list file.
fn read_list(storage: &dyn Storage, path: &str, max: usize) -> Vec<String> {
    let Ok(data) = crate::read_file(storage, path, max * 512) else {
        return Vec::new();
    };
    core::str::from_utf8(&data)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// What the library knows of the book at `path`, if it belongs to this
/// file (not to another one of that name before).
pub fn book(storage: &dyn Storage, path: &str) -> Option<Book> {
    let size = storage.size(path).ok()?;
    let data = crate::read_file(storage, &book_file(path), 8192).ok()?;
    let source = core::str::from_utf8(&data).ok()?;
    let mut book = Book::default();
    let (mut cover_path, mut cover_width, mut cover_height) = (None, None, None);
    let mut version = 1;
    for entry in config::parse(source).filter_map(Result::ok) {
        let text = || entry.value.as_str().into_owned();
        match (entry.section, entry.key) {
            ("field", key) => book.fields.push((key.to_string(), text())),
            (_, "size") => book.size = entry.value.raw().parse().unwrap_or_default(),
            (_, "indexed") => book.indexed = entry.value.as_bool().unwrap_or_default(),
            (_, "title") => book.title = Some(text()),
            (_, "author") => book.author = Some(text()),
            (_, "series") => book.series = Some(text()),
            (_, "series-index") => book.series_index = Some(text()),
            (_, key) if key.starts_with("tag.") => book.tags.push(text()),
            (_, "version") => version = entry.value.as_u32().unwrap_or(1),
            (_, "cover") => cover_path = Some(text()),
            (_, "cover-width") => cover_width = entry.value.as_u32(),
            (_, "cover-height") => cover_height = entry.value.as_u32(),
            (_, "progress") => book.progress = entry.value.as_u32().map(|p| p.min(100) as u8),
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
    // Details of an older kind are read again.
    book.indexed &= version == DETAILS;
    (book.size == size).then_some(book)
}

/// Keeps `book` as what the library knows of the book at `path`.
pub fn save(storage: &dyn Storage, path: &str, book: &Book) {
    let mut ini = String::new();
    let mut set = |key: &str, value: &str| ini = config::set(&ini, "book", key, value);
    set("size", &book.size.to_string());
    set("indexed", if book.indexed { "on" } else { "off" });
    set("version", &DETAILS.to_string());
    if let Some(title) = &book.title {
        set("title", title);
    }
    if let Some(author) = &book.author {
        set("author", author);
    }
    if let Some(series) = &book.series {
        set("series", series);
    }
    if let Some(index) = &book.series_index {
        set("series-index", index);
    }
    for (i, tag) in book.tags.iter().enumerate() {
        set(&format!("tag.{}", i + 1), tag);
    }
    if let Some(cover) = &book.cover {
        set("cover", &cover.path);
        set("cover-width", &cover.width.to_string());
        set("cover-height", &cover.height.to_string());
    }
    if let Some(progress) = book.progress {
        set("progress", &progress.to_string());
    }
    for (id, value) in &book.fields {
        ini = config::set(&ini, "field", id, value);
    }
    let _ = storage.write(&book_file(path), ini.as_bytes());
}

/// What the library knows of the book at `path`, or a new entry for it.
fn book_or_new(storage: &dyn Storage, path: &str) -> Book {
    book(storage, path).unwrap_or_else(|| Book {
        size: storage.size(path).unwrap_or_default(),
        ..Book::default()
    })
}

/// Reads title, author, series, tags and cover from the book itself and
/// keeps them. Opening a book the first time may take a moment (e.g. an
/// EPUB's package is read), so views do this for one book at a time,
/// e.g. for the rows they show.
pub fn index(env: &Env<'_>, path: &str) -> Book {
    let storage = env.storage;
    let mut book = book_or_new(storage, path);
    book.size = storage.size(path).unwrap_or_default();
    book.indexed = true;
    if let Ok(document) = env.formats.open(storage, path) {
        let metadata = document.metadata();
        book.title = metadata.title.clone();
        book.author = metadata.authors.first().cloned();
        book.series = metadata.series.clone();
        book.series_index = metadata.series_index.clone();
        book.tags = metadata.subjects.clone();
        book.cover = document.cover(storage).ok().flatten();
    }
    save(storage, path, &book);
    book
}

/// What the library knows of the book at `path`, read from the book
/// first if it is not yet ([`index`]).
pub fn indexed(env: &Env<'_>, path: &str) -> Book {
    match book(env.storage, path) {
        Some(book) if book.indexed => book,
        _ => index(env, path),
    }
}

/// Sets field `id` of the book at `path` (a plugin's, e.g.
/// `myplugin.rating`); an empty value removes it.
pub fn set_field(storage: &dyn Storage, path: &str, id: &str, value: &str) {
    let mut book = book_or_new(storage, path);
    book.fields.retain(|(k, _)| k != id);
    if !value.is_empty() {
        book.fields.push((id.to_string(), value.to_string()));
    }
    save(storage, path, &book);
}

/// Records a reading event (`core.book-opened`, `core.page-turned`): the
/// book's progress, its title until it is read from the book, and for an
/// opened book its place in the "recently read" order.
pub fn record(env: &Env<'_>, params: &Params, opened: bool) {
    let Some(path) = params.get(params::PATH) else {
        return;
    };
    let storage = env.storage;
    if opened {
        let mut list = opened_books(storage);
        list.retain(|p| p != path);
        list.insert(0, path.to_string());
        list.truncate(MAX_OPENED);
        let _ = storage.write(&opened_file(), list.join("\n").as_bytes());
    }
    let progress = params
        .get(params::PROGRESS)
        .and_then(|p| p.parse::<f32>().ok())
        .map(|p| (p + 0.5).clamp(0.0, 100.0) as u8);
    let mut book = book_or_new(storage, path);
    let mut changed = false;
    if let Some(title) = params.get(params::TITLE)
        && !book.indexed
        && book.title.as_deref() != Some(title)
    {
        book.title = Some(title.to_string());
        changed = true;
    }
    // Written only when the shown percentage changes.
    if progress.is_some() && book.progress != progress {
        book.progress = progress;
        changed = true;
    }
    if changed {
        save(storage, path, &book);
    }
}

/// Moves what the library knows of a book from `from` to `to`: its
/// details, its place among the recently opened and the favourites.
pub fn moved(storage: &dyn Storage, from: &str, to: &str) {
    let _ = storage.rename(&book_file(from), &book_file(to));
    let replace = |list: Vec<String>| -> Vec<String> {
        list.into_iter()
            .map(|p| if p == from { to.to_string() } else { p })
            .collect()
    };
    let opened = replace(opened_books(storage));
    let _ = storage.write(&opened_file(), opened.join("\n").as_bytes());
    let favourites = replace(favourites(storage));
    let _ = storage.write(&favourites_file(), favourites.join("\n").as_bytes());
}

/// Marks the book at `path` as read to the end, or as not read at all.
pub fn set_finished(storage: &dyn Storage, path: &str, finished: bool) {
    let mut book = book_or_new(storage, path);
    book.progress = finished.then_some(100);
    save(storage, path, &book);
}

/// Paths of the books opened last, most recent first (also of books no
/// longer there).
pub fn opened_books(storage: &dyn Storage) -> Vec<String> {
    read_list(storage, &opened_file(), MAX_OPENED)
}

/// The paths of the favourite books, the newest first.
pub fn favourites(storage: &dyn Storage) -> Vec<String> {
    read_list(storage, &favourites_file(), MAX_FAVOURITES)
}

/// Makes the book at `path` a favourite, or no longer one.
pub fn set_favourite(storage: &dyn Storage, path: &str, favourite: bool) {
    let mut list = favourites(storage);
    list.retain(|p| p != path);
    if favourite {
        list.insert(0, path.to_string());
        list.truncate(MAX_FAVOURITES);
    }
    let _ = storage.write(&favourites_file(), list.join("\n").as_bytes());
}

/// Takes over what the library plugin kept before the library was a
/// service (`.quireink/plugins/files`), once.
pub fn take_over_old(storage: &dyn Storage) {
    if storage.exists(&opened_file()) || storage.exists(&favourites_file()) {
        return;
    }
    let old = crate::Env::plugin_dir("files");
    for name in ["opened.txt", "favourites.txt"] {
        let _ = storage.rename(&format!("{old}/{name}"), &format!("{DIR}/{name}"));
    }
    let _ = storage.rename(&format!("{old}/books"), &format!("{DIR}/books"));
    // The catalog is made anew where it is now.
    let _ = storage.remove(&format!("{old}/catalog.txt"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStorage;

    #[test]
    fn details_belong_to_one_file_version() {
        let storage = MemoryStorage::new().with("books/a.txt", "hello");
        let book = Book {
            size: 5,
            indexed: true,
            title: Some("A = \"title\"".into()),
            author: Some("Ann".into()),
            series: Some("Tales".into()),
            series_index: Some("2".into()),
            tags: alloc::vec!["Fantasy".into(), "To read".into()],
            cover: Some(ImageRef {
                path: "x.gray".into(),
                width: 30,
                height: 40,
            }),
            progress: Some(12),
            fields: alloc::vec![("stars.rating".into(), "4".into())],
        };
        save(&storage, "books/a.txt", &book);
        let read = super::book(&storage, "books/a.txt");
        assert_eq!(read, Some(book));
        assert_eq!(read.unwrap().field("stars.rating"), Some("4"));
        set_field(&storage, "books/a.txt", "stars.rating", "");
        assert!(
            super::book(&storage, "books/a.txt")
                .unwrap()
                .fields
                .is_empty()
        );
        // Replaced by a file of another size: forgotten.
        let storage = storage.with("books/a.txt", "hello!");
        assert_eq!(super::book(&storage, "books/a.txt"), None);
    }

    #[test]
    fn favourites_newest_first() {
        let storage = MemoryStorage::new();
        set_favourite(&storage, "a.epub", true);
        set_favourite(&storage, "b.epub", true);
        set_favourite(&storage, "a.epub", true);
        assert_eq!(favourites(&storage), ["a.epub", "b.epub"]);
        set_favourite(&storage, "a.epub", false);
        assert_eq!(favourites(&storage), ["b.epub"]);
        moved(&storage, "b.epub", "c.epub");
        assert_eq!(favourites(&storage), ["c.epub"]);
    }

    #[test]
    fn titles_fall_back_to_the_file_name() {
        let book = Book::default();
        assert_eq!(
            book.title_or("books/Alice in Wonderland.epub"),
            "Alice in Wonderland"
        );
        let book = Book {
            title: Some("Alice".into()),
            ..Book::default()
        };
        assert_eq!(book.title_or("books/x.epub"), "Alice");
    }

    #[test]
    fn what_the_library_plugin_kept_is_taken_over() {
        let storage = MemoryStorage::new()
            .with(".quireink/plugins/files/opened.txt", "books/a.epub")
            .with(".quireink/plugins/files/books/0000000000000001.ini", "x");
        take_over_old(&storage);
        assert_eq!(opened_books(&storage), ["books/a.epub"]);
        assert!(storage.exists(".quireink/library/books/0000000000000001.ini"));
    }
}

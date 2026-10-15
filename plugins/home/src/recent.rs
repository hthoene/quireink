//! The list of recently read books, kept up to date from the standard
//! reading events, independent of which reader plugin is used.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::params;
use quireink_sdk::{Env, Params, Storage};

use crate::PLUGIN;

/// Books remembered.
const LIMIT: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recent {
    pub path: String,
    pub title: String,
    /// Percent, 0 to 100.
    pub progress: f32,
}

fn file() -> String {
    alloc::format!("{}/recent.ini", Env::plugin_dir(PLUGIN.as_str()))
}

pub(crate) fn load(storage: &dyn Storage) -> Vec<Recent> {
    let Ok(data) = quireink_sdk::read_file(storage, &file(), 16 * 1024) else {
        return Vec::new();
    };
    let Ok(text) = core::str::from_utf8(&data) else {
        return Vec::new();
    };
    let mut books: Vec<Recent> = Vec::new();
    let mut section = "";
    for entry in quireink_sdk::config::parse(text).filter_map(Result::ok) {
        if entry.section != section {
            section = entry.section;
            books.push(Recent {
                path: String::new(),
                title: String::new(),
                progress: 0.0,
            });
        }
        let Some(book) = books.last_mut() else {
            continue;
        };
        let value = entry.value.as_str();
        match entry.key {
            "path" => book.path = value.to_string(),
            "title" => book.title = value.to_string(),
            "progress" => book.progress = value.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    books.retain(|b| !b.path.is_empty());
    books
}

fn save(storage: &dyn Storage, books: &[Recent]) {
    let mut text = String::new();
    for (i, book) in books.iter().enumerate() {
        text.push_str(&alloc::format!(
            "[book.{}]\npath = {}\ntitle = {}\nprogress = {:.1}\n\n",
            i + 1,
            quote(&book.path),
            quote(&book.title),
            book.progress
        ));
    }
    let _ = storage.write(&file(), text.as_bytes());
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A book was moved: its entry follows.
pub(crate) fn moved(env: &Env<'_>, event: &Params) {
    let (Some(from), Some(to)) = (event.get(params::FROM), event.get(params::PATH)) else {
        return;
    };
    let mut books = load(env.storage);
    let mut changed = false;
    for book in books.iter_mut().filter(|b| b.path == from) {
        book.path = to.to_string();
        changed = true;
    }
    if changed {
        save(env.storage, &books);
    }
}

/// Moves the book of a reading event to the top of the list.
pub(crate) fn record(env: &Env<'_>, event: &Params) {
    let Some(path) = event.get(params::PATH) else {
        return;
    };
    let mut books = load(env.storage);
    let previous = books
        .iter()
        .position(|b| b.path == path)
        .map(|i| books.remove(i));
    let title = event
        .get(params::TITLE)
        .map(ToString::to_string)
        .or(previous.map(|b| b.title))
        .unwrap_or_else(|| path.to_string());
    let progress = event
        .get(params::PROGRESS)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0.0);
    books.insert(
        0,
        Recent {
            path: path.to_string(),
            title,
            progress,
        },
    );
    books.truncate(LIMIT);
    save(env.storage, &books);
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn saves_loads_and_quotes() {
        let storage = MemoryStorage::new();
        let books = [
            Recent {
                path: String::from("books/a \"b\".txt"),
                title: String::from("A \\ B"),
                progress: 12.25,
            },
            Recent {
                path: String::from("books/c.txt"),
                title: String::from("C"),
                progress: 0.0,
            },
        ];
        save(&storage, &books);
        let loaded = load(&storage);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].path, "books/a \"b\".txt");
        assert_eq!(loaded[0].title, "A \\ B");
        assert!(
            (loaded[0].progress - 12.3).abs() < 0.01 || (loaded[0].progress - 12.2).abs() < 0.01
        );
        assert_eq!(loaded[1].title, "C");
    }
}

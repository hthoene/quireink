//! The bookmarks and highlights of books, shared by plugins: the reader
//! makes and shows them, others export or sync them (e.g. with KOReader).
//! Whoever changes those of a book emits
//! [`ANNOTATIONS_CHANGED`](events::ANNOTATIONS_CHANGED)
//! (see [`changed`]), so readers show them anew and syncs pass them on.
//!
//! A book's annotations are kept in `.quireink/annotations/<hash>.txt`
//! (the hash of the book's path), one per line, sorted by position: kind
//! (`b` or `h`), start and end (section and offset, so they outlast font
//! sizes and layouts), the reading progress, when it was made and last
//! changed (seconds since 1970, 0 if not known), a snippet of the text
//! and the note on it, separated by tabs.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::builtin::{events, params};
use crate::document::Locator;
use crate::{Command, Params, Storage};

/// Longest snippet kept with an annotation, in bytes.
const MAX_TEXT: usize = 240;
/// Longest annotations file read.
const MAX_FILE: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bookmark,
    Highlight,
}

/// Where an annotation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub kind: Kind,
    /// For a highlight, its first character; for a bookmark, where the
    /// page begins.
    pub start: Locator,
    /// Just after the highlight's last character.
    pub end: Locator,
    /// It has a note.
    pub noted: bool,
}

impl Mark {
    /// Whether the mark covers part of `start..end`.
    pub fn overlaps(&self, start: Locator, end: Locator) -> bool {
        self.start < end && start < self.end
    }
}

/// An annotation with what is shown of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub mark: Mark,
    /// Reading progress at the start, in percent.
    pub percent: u8,
    /// When it was made and last changed, in seconds since 1970 (UTC); 0
    /// if not known.
    pub created: i64,
    pub updated: i64,
    /// A snippet of the text (see [`snippet`]).
    pub text: String,
    /// What the reader noted on it, if anything (one line).
    pub note: String,
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The file with the annotations of the book at `book`.
pub fn file(book: &str) -> String {
    format!(".quireink/annotations/{:016x}.txt", fnv(book))
}

/// Every annotation of the book at `book`, by position.
pub fn load(storage: &dyn Storage, book: &str) -> Vec<Annotation> {
    let Ok(data) = crate::read_file(storage, &file(book), MAX_FILE) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(parse)
        .collect()
}

/// Only where the annotations are, for drawing them.
pub fn marks(storage: &dyn Storage, book: &str) -> Vec<Mark> {
    load(storage, book).into_iter().map(|a| a.mark).collect()
}

fn parse(line: &str) -> Option<Annotation> {
    let fields: Vec<&str> = line.splitn(10, '\t').collect();
    let kind = match *fields.first()? {
        "b" => Kind::Bookmark,
        "h" => Kind::Highlight,
        _ => return None,
    };
    let number = |i: usize| fields.get(i)?.parse::<i64>().ok();
    let locator = |i: usize| {
        Some(Locator::new(
            u32::try_from(number(i)?).ok()?,
            u32::try_from(number(i + 1)?).ok()?,
        ))
    };
    let (start, end) = (locator(1)?, locator(3)?);
    let percent = number(5)?.clamp(0, 100) as u8;
    // Before times were kept, the text followed the progress.
    let (created, updated, rest) = if fields.len() == 10 {
        (number(6)?, number(7)?, 8)
    } else {
        (0, 0, 6)
    };
    let text = fields.get(rest).copied().unwrap_or_default().to_string();
    let note = fields
        .get(rest + 1)
        .copied()
        .unwrap_or_default()
        .to_string();
    Some(Annotation {
        mark: Mark {
            kind,
            start,
            end,
            noted: !note.is_empty(),
        },
        percent,
        created,
        updated,
        text,
        note,
    })
}

/// Keeps `annotations` as those of the book at `book`, sorted by
/// position.
pub fn save(storage: &dyn Storage, book: &str, annotations: &[Annotation]) {
    let mut all: Vec<&Annotation> = annotations.iter().collect();
    all.sort_by_key(|a| (a.mark.start, a.mark.end));
    let mut out = String::new();
    for a in all {
        let kind = match a.mark.kind {
            Kind::Bookmark => 'b',
            Kind::Highlight => 'h',
        };
        out.push_str(&format!(
            "{kind}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            a.mark.start.section,
            a.mark.start.offset,
            a.mark.end.section,
            a.mark.end.offset,
            a.percent,
            a.created,
            a.updated,
            one_line(&a.text),
            one_line(&a.note)
        ));
    }
    let path = file(book);
    if out.is_empty() {
        let _ = storage.remove(&path);
    } else {
        let _ = storage.write(&path, out.as_bytes());
    }
}

/// `text` on one line, at most 240 bytes.
pub fn snippet(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    if out.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

/// `text` on one line: tabs and line breaks become spaces.
pub fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Puts `note` on the highlight that starts at `start` (an empty one
/// removes it), changed at `now` (seconds since 1970, 0 if not known);
/// `false` if there is no such highlight.
pub fn set_note(storage: &dyn Storage, book: &str, start: Locator, note: &str, now: i64) -> bool {
    let mut all = load(storage, book);
    let Some(a) = all
        .iter_mut()
        .find(|a| a.mark.kind == Kind::Highlight && a.mark.start == start)
    else {
        return false;
    };
    a.note = one_line(note);
    a.mark.noted = !a.note.is_empty();
    a.updated = now;
    save(storage, book, &all);
    true
}

/// The note on the highlight that starts at `start`.
pub fn note_of(storage: &dyn Storage, book: &str, start: Locator) -> String {
    load(storage, book)
        .into_iter()
        .find(|a| a.mark.kind == Kind::Highlight && a.mark.start == start)
        .map(|a| a.note)
        .unwrap_or_default()
}

/// Adds an annotation.
pub fn add(storage: &dyn Storage, book: &str, annotation: Annotation) {
    let mut all = load(storage, book);
    all.push(annotation);
    save(storage, book, &all);
}

/// Removes the annotations `remove` picks; `true` if there were any.
pub fn remove(storage: &dyn Storage, book: &str, remove: impl Fn(&Mark) -> bool) -> bool {
    let mut all = load(storage, book);
    let before = all.len();
    all.retain(|a| !remove(&a.mark));
    let removed = all.len() != before;
    if removed {
        save(storage, book, &all);
    }
    removed
}

/// The annotations of a book moved with it from `from` to `to`.
pub fn moved(storage: &dyn Storage, from: &str, to: &str) {
    let _ = storage.rename(&file(from), &file(to));
}

/// The event telling that the annotations of the book at `book` changed.
pub fn changed(book: &str) -> Command {
    Command::Emit(
        events::ANNOTATIONS_CHANGED,
        Params::new().with(params::PATH, book.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStorage;

    fn highlight(start: u32, end: u32, text: &str) -> Annotation {
        Annotation {
            mark: Mark {
                kind: Kind::Highlight,
                start: Locator::new(1, start),
                end: Locator::new(1, end),
                noted: false,
            },
            percent: 3,
            created: 1_700_000_000,
            updated: 0,
            text: String::from(text),
            note: String::new(),
        }
    }

    #[test]
    fn kept_by_position_with_notes_and_times() {
        let storage = MemoryStorage::new();
        let book = "books/a.epub";
        add(&storage, book, highlight(50, 60, "later"));
        add(&storage, book, highlight(10, 20, "first\tline"));
        assert!(set_note(
            &storage,
            book,
            Locator::new(1, 50),
            "a\nnote",
            1_700_000_100
        ));
        let all = load(&storage, book);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].text, "first line");
        assert_eq!(all[1].note, "a note");
        assert!(all[1].mark.noted);
        assert_eq!(
            (all[1].created, all[1].updated),
            (1_700_000_000, 1_700_000_100)
        );
        assert_eq!(note_of(&storage, book, Locator::new(1, 50)), "a note");
        // Moved along with the book; gone when the last is removed.
        moved(&storage, book, "books/b.epub");
        assert!(load(&storage, book).is_empty());
        assert!(remove(&storage, "books/b.epub", |_| true));
        assert!(!storage.exists(&file("books/b.epub")));
    }

    #[test]
    fn lines_without_times_are_read() {
        let a = parse("h\t1\t10\t1\t20\t5\tsome text\tits note").unwrap();
        assert_eq!((a.created, a.updated), (0, 0));
        assert_eq!(
            (a.text.as_str(), a.note.as_str()),
            ("some text", "its note")
        );
        assert_eq!(a.mark.end, Locator::new(1, 20));
    }
}

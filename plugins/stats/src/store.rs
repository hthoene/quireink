//! Where the statistics live, in KOReader's data model so they can be
//! exchanged with its statistics:
//!
//! - `books.ini`: the books, one section `[book.<id>]` each, with title,
//!   authors (separated by line breaks, as KOReader does), the partial
//!   MD5 of the file, pages, language, series, when it was last opened and
//!   its path here.
//! - `pages.bin`: every stay on a page, 16 bytes each, appended as they
//!   happen: book id (u16), page (u32, from 1), start (u32, Unix seconds),
//!   duration (u16, seconds) and the book's pages then (u32), like KOReader's
//!   `page_stat_data`.
//! - `days.bin`: reading time per local day for quick views, derived from
//!   `pages.bin`: how many bytes of the log it covers (u64), then 12 bytes
//!   per day, in order: the day (i32, days since 1970), seconds (u32) and
//!   stays (u32). Brought up to date when a view needs it, so reading only
//!   ever appends 16 bytes per page.
//!
//! Nothing is held in memory but what a view shows: the log is read in
//! small chunks, from the end for recent days.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::{Env, Storage, StorageError};

/// A stay on a page (KOReader: a row of `page_stat_data`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageStat {
    pub book: u16,
    /// From 1.
    pub page: u32,
    /// Unix seconds.
    pub start: u32,
    /// Seconds.
    pub duration: u16,
    /// The book's pages when it was read.
    pub pages: u32,
}

const RECORD: usize = 16;

impl PageStat {
    fn encode(&self) -> [u8; RECORD] {
        let mut out = [0u8; RECORD];
        out[0..2].copy_from_slice(&self.book.to_le_bytes());
        out[2..6].copy_from_slice(&self.page.to_le_bytes());
        out[6..10].copy_from_slice(&self.start.to_le_bytes());
        out[10..12].copy_from_slice(&self.duration.to_le_bytes());
        out[12..16].copy_from_slice(&self.pages.to_le_bytes());
        out
    }

    fn decode(b: &[u8]) -> Self {
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Self {
            book: u16::from_le_bytes([b[0], b[1]]),
            page: u32_at(2),
            start: u32_at(6),
            duration: u16::from_le_bytes([b[10], b[11]]),
            pages: u32_at(12),
        }
    }
}

/// A book (KOReader: a row of `book`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Book {
    pub id: u16,
    pub title: String,
    /// Separated by line breaks.
    pub authors: String,
    /// KOReader's partial MD5 of the file.
    pub md5: String,
    pub pages: u32,
    pub language: String,
    pub series: String,
    /// Unix seconds.
    pub last_open: i64,
    /// Where it is on this device (empty if it is not).
    pub path: String,
}

/// Reading on a local day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Day {
    /// Days since 1970-01-01, local time.
    pub day: i32,
    pub seconds: u32,
    /// Stays on pages (a page read twice counts twice).
    pub stays: u32,
}

const DAY: usize = 12;

impl Day {
    fn encode(&self) -> [u8; DAY] {
        let mut out = [0u8; DAY];
        out[0..4].copy_from_slice(&self.day.to_le_bytes());
        out[4..8].copy_from_slice(&self.seconds.to_le_bytes());
        out[8..12].copy_from_slice(&self.stays.to_le_bytes());
        out
    }

    fn decode(b: &[u8]) -> Self {
        let at = |i: usize| [b[i], b[i + 1], b[i + 2], b[i + 3]];
        Self {
            day: i32::from_le_bytes(at(0)),
            seconds: u32::from_le_bytes(at(4)),
            stays: u32::from_le_bytes(at(8)),
        }
    }
}

fn dir() -> String {
    Env::plugin_dir(crate::PLUGIN.as_str())
}

fn path(name: &str) -> String {
    format!("{}/{name}", dir())
}

/// A value for the configuration format, quoted with escapes.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The books, by id.
pub(crate) fn books(storage: &dyn Storage) -> Vec<Book> {
    let Ok(data) = quireink_sdk::read_file(storage, &path("books.ini"), 1 << 20) else {
        return Vec::new();
    };
    let text = core::str::from_utf8(&data).unwrap_or_default();
    let mut books: Vec<Book> = Vec::new();
    for entry in quireink_sdk::config::parse(text).filter_map(Result::ok) {
        let Some(id) = entry
            .section
            .strip_prefix("book.")
            .and_then(|id| id.parse::<u16>().ok())
        else {
            continue;
        };
        if books.last().is_none_or(|b| b.id != id) {
            books.push(Book {
                id,
                ..Book::default()
            });
        }
        let book = books.last_mut().unwrap_or_else(|| unreachable!());
        let value = entry.value.as_str().into_owned();
        match entry.key {
            "title" => book.title = value,
            "authors" => book.authors = value,
            "md5" => book.md5 = value,
            "pages" => book.pages = value.parse().unwrap_or(0),
            "language" => book.language = value,
            "series" => book.series = value,
            "last-open" => book.last_open = value.parse().unwrap_or(0),
            "path" => book.path = value,
            _ => {}
        }
    }
    books.sort_by_key(|b| b.id);
    books.dedup_by_key(|b| b.id);
    books
}

pub(crate) fn save_books(storage: &dyn Storage, books: &[Book]) {
    let mut text = String::new();
    for b in books {
        text.push_str(&format!(
            "[book.{}]\ntitle = {}\nauthors = {}\nmd5 = {}\npages = {}\nlanguage = {}\nseries = {}\nlast-open = {}\npath = {}\n\n",
            b.id,
            quoted(&b.title),
            quoted(&b.authors),
            b.md5,
            b.pages,
            quoted(&b.language),
            quoted(&b.series),
            b.last_open,
            quoted(&b.path),
        ));
    }
    let _ = storage.write(&path("books.ini"), text.as_bytes());
}

/// Records a stay on a page.
pub(crate) fn append(storage: &dyn Storage, stat: PageStat) {
    let _ = storage.append(&path("pages.bin"), &stat.encode());
}

/// Replaces the log with the stays in `file` (records of book, page,
/// start, duration and pages as big-endian u32, in time order), e.g.
/// after merging with another device's; the day totals are summed anew.
pub(crate) fn replace_log(storage: &dyn Storage, file: &str) -> Result<(), StorageError> {
    let new = path("pages.new");
    storage.write(&new, &[])?;
    let mut chunk = [0u8; 20 * 64];
    let mut at = 0u64;
    loop {
        let n = storage.read_at(file, at, &mut chunk)?;
        let whole = n - n % 20;
        let mut out = Vec::with_capacity(whole / 20 * RECORD);
        for r in chunk[..whole].chunks_exact(20) {
            let v = |i: usize| u32::from_be_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
            let stat = PageStat {
                book: v(0).min(u32::from(u16::MAX)) as u16,
                page: v(4),
                start: v(8),
                duration: v(12).min(u32::from(u16::MAX)) as u16,
                pages: v(16),
            };
            out.extend_from_slice(&stat.encode());
        }
        storage.append(&new, &out)?;
        if whole == 0 || n < chunk.len() {
            break;
        }
        at += whole as u64;
    }
    // Swapped in at once (by a rename where the storage can), then the
    // day totals go.
    if storage.rename(&new, &path("pages.bin")).is_err() {
        let data =
            quireink_sdk::read_file(storage, &new, 256 << 20).map_err(|_| StorageError::Io)?;
        storage.write(&path("pages.bin"), &data)?;
        let _ = storage.remove(&new);
    }
    let _ = storage.remove(&path("days.bin"));
    Ok(())
}

/// Every day with reading, in order, local days as `day_of` makes them
/// from Unix seconds; brings `days.bin` up to date with the log first.
pub(crate) fn days(storage: &dyn Storage, day_of: impl Fn(u32) -> i32) -> Vec<Day> {
    let file = path("days.bin");
    let data = quireink_sdk::read_file(storage, &file, 1 << 20).unwrap_or_default();
    let (mut covered, mut days) = match data.split_first_chunk::<8>() {
        Some((head, rest)) => (
            u64::from_le_bytes(*head),
            rest.chunks_exact(DAY).map(Day::decode).collect::<Vec<_>>(),
        ),
        None => (0, Vec::new()),
    };
    let log = path("pages.bin");
    let size = storage.size(&log).unwrap_or(0);
    if covered > size {
        // The log was replaced (e.g. merged with another device's).
        covered = 0;
        days.clear();
    }
    if covered + RECORD as u64 > size {
        return days;
    }
    let mut chunk = [0u8; RECORD * 64];
    let mut at = covered;
    while let Ok(n) = storage.read_at(&log, at, &mut chunk) {
        let whole = n - n % RECORD;
        for record in chunk[..whole].chunks_exact(RECORD) {
            let stat = PageStat::decode(record);
            let day = day_of(stat.start);
            match days.binary_search_by_key(&day, |d| d.day) {
                Ok(i) => {
                    days[i].seconds += u32::from(stat.duration);
                    days[i].stays += 1;
                }
                Err(i) => days.insert(
                    i,
                    Day {
                        day,
                        seconds: u32::from(stat.duration),
                        stays: 1,
                    },
                ),
            }
        }
        at += whole as u64;
        if n < chunk.len() || whole == 0 {
            break;
        }
    }
    let mut out = Vec::with_capacity(8 + days.len() * DAY);
    out.extend_from_slice(&at.to_le_bytes());
    for d in &days {
        out.extend_from_slice(&d.encode());
    }
    let _ = storage.write(&file, &out);
    days
}

/// Calls `visit` for every stay, oldest first.
pub(crate) fn scan(storage: &dyn Storage, mut visit: impl FnMut(&PageStat)) {
    let file = path("pages.bin");
    let mut chunk = [0u8; RECORD * 64];
    let mut at = 0u64;
    loop {
        let Ok(n) = storage.read_at(&file, at, &mut chunk) else {
            return;
        };
        let whole = n - n % RECORD;
        for record in chunk[..whole].chunks_exact(RECORD) {
            visit(&PageStat::decode(record));
        }
        if n < chunk.len() || whole == 0 {
            return;
        }
        at += whole as u64;
    }
}

/// Calls `visit` for the stays that started at `since` (Unix seconds) or
/// later, newest first; reads only the end of the log, as stays are
/// recorded in order.
pub(crate) fn scan_recent(storage: &dyn Storage, since: u32, mut visit: impl FnMut(&PageStat)) {
    let file = path("pages.bin");
    let size = storage.size(&file).unwrap_or(0);
    let mut end = size - size % RECORD as u64;
    let mut chunk = [0u8; RECORD * 64];
    // Stays a little older may come later (a page read before a pause).
    let margin = 3600;
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        let len = (end - start) as usize;
        if storage.read_at(&file, start, &mut chunk[..len]).ok() != Some(len) {
            return;
        }
        let mut older = false;
        for record in chunk[..len].chunks_exact(RECORD).rev() {
            let stat = PageStat::decode(record);
            if stat.start >= since {
                visit(&stat);
            } else if stat.start + margin < since {
                older = true;
            }
        }
        if older {
            return;
        }
        end = start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    fn stat(book: u16, page: u32, start: u32, duration: u16) -> PageStat {
        PageStat {
            book,
            page,
            start,
            duration,
            pages: 300,
        }
    }

    #[test]
    fn books_survive_the_file() {
        let storage = MemoryStorage::new();
        let books_in = [
            Book {
                id: 1,
                title: String::from("Fjord \"1\""),
                authors: String::from("A. Author\nB. Author"),
                md5: String::from("abc"),
                pages: 614,
                language: String::from("no"),
                series: String::from("N/A"),
                last_open: 1_760_000_000,
                path: String::from("books/d.epub"),
            },
            Book {
                id: 2,
                title: String::from("Zwei"),
                ..Book::default()
            },
        ];
        save_books(&storage, &books_in);
        assert_eq!(books(&storage), books_in);
    }

    #[test]
    fn stays_are_logged_and_summed_per_day() {
        let storage = MemoryStorage::new();
        // A day of 100 seconds, for short numbers.
        let day_of = |unix: u32| (unix / 100) as i32;
        append(&storage, stat(1, 1, 1000, 30));
        append(&storage, stat(1, 2, 1030, 40));
        append(&storage, stat(2, 7, 1100, 20));
        assert_eq!(
            days(&storage, day_of),
            [
                Day {
                    day: 10,
                    seconds: 70,
                    stays: 2
                },
                Day {
                    day: 11,
                    seconds: 20,
                    stays: 1
                },
            ]
        );
        // Later stays are added to what was summed, also earlier days.
        append(&storage, stat(1, 3, 950, 10));
        append(&storage, stat(1, 4, 1150, 5));
        assert_eq!(
            days(&storage, day_of),
            [
                Day {
                    day: 9,
                    seconds: 10,
                    stays: 1
                },
                Day {
                    day: 10,
                    seconds: 70,
                    stays: 2
                },
                Day {
                    day: 11,
                    seconds: 25,
                    stays: 2
                },
            ]
        );
        // A replaced, shorter log is summed anew.
        let _ = storage.write(&path("pages.bin"), &stat(3, 1, 1200, 9).encode());
        assert_eq!(
            days(&storage, day_of),
            [Day {
                day: 12,
                seconds: 9,
                stays: 1
            }]
        );
        let mut all = Vec::new();
        scan(&storage, |s| all.push(*s));
        assert_eq!(all, [stat(3, 1, 1200, 9)]);
    }

    #[test]
    fn recent_stays_come_from_the_end() {
        let storage = MemoryStorage::new();
        for (i, start) in [100u32, 130, 90_000].into_iter().enumerate() {
            append(&storage, stat(1, i as u32 + 1, start, 10));
        }
        let mut recent = Vec::new();
        scan_recent(&storage, 120, |s| recent.push(s.start));
        assert_eq!(recent, [90_000, 130]);
    }

    #[test]
    fn long_logs_are_read_in_chunks() {
        let storage = MemoryStorage::new();
        for i in 0..200u32 {
            let _ = storage.append(
                &path("pages.bin"),
                &stat(1, i + 1, 1000 + i * 10, 9).encode(),
            );
        }
        let mut count = 0;
        scan(&storage, |_| count += 1);
        assert_eq!(count, 200);
        let mut recent = 0;
        scan_recent(&storage, 1000 + 150 * 10, |_| recent += 1);
        assert_eq!(recent, 50);
    }
}

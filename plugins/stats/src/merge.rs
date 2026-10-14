//! Merging statistics with KOReader's, as KOReader's own sync does
//! (`ReaderStatistics.onSync`):
//!
//! - Books are the same when their file is (the partial MD5). Their title
//!   and authors are taken from KOReader, so KOReader recognises them by
//!   its key (title, authors, MD5) when it merges the file back.
//! - What the last uploaded file (`cached`) has but this device no longer
//!   has was deleted here: it is not taken from the other side. What it
//!   has but the other side no longer has was deleted there: it goes here
//!   too. Books and single stays alike.
//! - The same stay (book, page, start) on both sides keeps the longer
//!   duration.
//!
//! Everything goes through files sorted in small runs ([`extsort`]), so
//! memory stays small whatever the number of stays.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::sqlite::{Database, Error};

use crate::koreader::{self, KoBook, STAT, fields, stat_record};
use crate::store::{self, Book};
use crate::{extsort, totals};

/// What a merge did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Outcome {
    pub books: usize,
    pub stays: usize,
}

/// The other side's books and stays, by book id there.
struct Side {
    books: Vec<KoBook>,
    path: String,
}

fn open_side(storage: &dyn Storage, path: Option<&str>) -> Option<Side> {
    let path = path?;
    let db = Database::open(storage, path).ok()?;
    let books = koreader::books(&db).ok()?;
    Some(Side {
        books,
        path: path.to_string(),
    })
}

/// Merges the statistics database `income` (another device's, if any)
/// into this device's statistics; `cached` is the file this device last
/// uploaded. With `out`, writes the merged statistics there as a KOReader
/// database. Working files go to `dir`.
pub(crate) fn merge(
    storage: &dyn Storage,
    income: Option<&str>,
    cached: Option<&str>,
    out: Option<&str>,
    dir: &str,
) -> Result<Outcome, Error> {
    let income = open_side(storage, income);
    let cached = open_side(storage, cached);
    let mut books = store::books(storage);
    let had = |md5: &str| books.iter().any(|b| b.md5 == md5);
    let in_side = |side: &Option<Side>, md5: &str| {
        side.as_ref()
            .is_some_and(|s| s.books.iter().any(|b| b.md5 == md5))
    };
    // Deleted here: in the last upload, not here (before adding any).
    let deleted_here: Vec<String> = cached
        .iter()
        .flat_map(|c| c.books.iter())
        .filter(|b| !had(&b.md5))
        .map(|b| b.md5.clone())
        .collect();
    // Deleted there: in the last upload, not in the other side's file.
    let deleted_there: Vec<String> = match &income {
        Some(_) => cached
            .iter()
            .flat_map(|c| c.books.iter())
            .filter(|b| !in_side(&income, &b.md5))
            .map(|b| b.md5.clone())
            .collect(),
        None => Vec::new(),
    };
    books.retain(|b| !deleted_there.contains(&b.md5));
    // Books from the other side: named as there, added if new here.
    if let Some(side) = &income {
        for theirs in &side.books {
            if deleted_here.contains(&theirs.md5) {
                continue;
            }
            match books.iter_mut().find(|b| b.md5 == theirs.md5) {
                Some(ours) => {
                    ours.title.clone_from(&theirs.title);
                    ours.authors.clone_from(&theirs.authors);
                    if ours.language == "N/A" || ours.language.is_empty() {
                        ours.language.clone_from(&theirs.language);
                    }
                    if ours.series == "N/A" || ours.series.is_empty() {
                        ours.series.clone_from(&theirs.series);
                    }
                    ours.last_open = ours.last_open.max(theirs.last_open);
                    if ours.pages == 0 {
                        ours.pages = theirs.pages.max(0) as u32;
                    }
                }
                None => {
                    let id = books.iter().map(|b| b.id).max().unwrap_or(0) + 1;
                    books.push(Book {
                        id,
                        title: theirs.title.clone(),
                        authors: theirs.authors.clone(),
                        md5: theirs.md5.clone(),
                        pages: theirs.pages.max(0) as u32,
                        language: theirs.language.clone(),
                        series: theirs.series.clone(),
                        last_open: theirs.last_open,
                        path: String::new(),
                    });
                }
            }
        }
    }
    books.sort_by_key(|b| b.id);
    // A side's book ids as ids here.
    let ids = |side: &Side| -> Vec<(i64, u16)> {
        side.books
            .iter()
            .filter_map(|theirs| {
                let ours = books.iter().find(|b| b.md5 == theirs.md5)?;
                Some((theirs.id, ours.id))
            })
            .collect()
    };

    let path = |name: &str| format!("{dir}/{name}");
    let (l, i, c, m, t) = (
        path("merge-local"),
        path("merge-income"),
        path("merge-cached"),
        path("merge-out"),
        path("merge-time"),
    );
    let runs = path("merge-run");
    let present = |id: u16| books.binary_search_by_key(&id, |b| b.id).is_ok();

    // This device's stays.
    let mut buffer = Vec::new();
    storage.write(&l, &[])?;
    let mut failed = None;
    store::scan(storage, |s| {
        if present(s.book) {
            buffer.extend_from_slice(&stat_record([
                u32::from(s.book),
                s.page,
                s.start,
                u32::from(s.duration),
                s.pages,
            ]));
            if buffer.len() >= 4096 {
                if let Err(e) = storage.append(&l, &buffer) {
                    failed = Some(e);
                }
                buffer.clear();
            }
        }
    });
    storage.append(&l, &buffer)?;
    if let Some(e) = failed {
        return Err(Error::Storage(e));
    }
    extsort::sort(storage, &l, &l, STAT, &runs)?;
    // The other side's and the last upload's, with ids from here.
    for (side, file) in [(&income, &i), (&cached, &c)] {
        storage.write(file, &[])?;
        let Some(side) = side else {
            continue;
        };
        let map = ids(side);
        let db = Database::open(storage, &side.path)?;
        let mut buffer = Vec::new();
        let mut failed = None;
        koreader::stats(&db, |s| {
            let Some((_, id)) = map.iter().find(|(theirs, _)| *theirs == s.book) else {
                return;
            };
            let clamp = |v: i64| v.clamp(0, i64::from(u32::MAX)) as u32;
            buffer.extend_from_slice(&stat_record([
                u32::from(*id),
                clamp(s.page),
                clamp(s.start),
                clamp(s.duration),
                clamp(s.pages),
            ]));
            if buffer.len() >= 4096 {
                if let Err(e) = storage.append(file, &buffer) {
                    failed = Some(e);
                }
                buffer.clear();
            }
        })?;
        storage.append(file, &buffer)?;
        if let Some(e) = failed {
            return Err(Error::Storage(e));
        }
        extsort::sort(storage, file, file, STAT, &runs)?;
    }

    // The three, side by side by (book, page, start).
    storage.write(&m, &[])?;
    let has_income = income.is_some();
    let mut sources = [
        Source::new(storage, &l),
        Source::new(storage, &i),
        Source::new(storage, &c),
    ];
    let key = |r: &Vec<u8>| -> [u8; 12] {
        let mut k = [0u8; 12];
        k.copy_from_slice(&r[..12]);
        k
    };
    let mut out_buffer = Vec::new();
    let mut stays = 0;
    while let Some(next) = sources.iter().filter_map(|s| s.peek().map(key)).min() {
        // Each side's record for this key, if any (duplicates within a
        // side are merged too).
        let mut found: [Option<Vec<u8>>; 3] = [None, None, None];
        for (n, source) in sources.iter_mut().enumerate() {
            while source.peek().is_some_and(|r| key(r) == next) {
                let r = source.next().unwrap_or_default();
                let longer = found[n]
                    .as_ref()
                    .is_none_or(|f| fields(&r)[3] > fields(f)[3]);
                if longer {
                    found[n] = Some(r);
                }
            }
        }
        let [here, there, before] = &found;
        let deleted_here = before.is_some() && here.is_none();
        let deleted_there = has_income && before.is_some() && there.is_none();
        if deleted_here || deleted_there {
            continue;
        }
        let record = match (here, there) {
            (Some(a), Some(b)) => {
                if fields(b)[3] > fields(a)[3] {
                    b.clone()
                } else {
                    a.clone()
                }
            }
            (Some(a), None) => a.clone(),
            (None, Some(b)) => b.clone(),
            (None, None) => continue,
        };
        // In time order next: start first.
        let [book, page, start, duration, pages] = fields(&record).map(|v| v as u32);
        out_buffer.extend_from_slice(&stat_record([start, book, page, duration, pages]));
        stays += 1;
        if out_buffer.len() >= 4096 {
            storage.append(&m, &out_buffer)?;
            out_buffer.clear();
        }
    }
    storage.append(&m, &out_buffer)?;
    for source in &mut sources {
        // A read error ends a source early: better no merge than a partial.
        if let Some(e) = source.records.failed.take() {
            return Err(Error::Storage(e));
        }
    }
    extsort::sort(storage, &m, &m, STAT, &runs)?;

    // Back to (book, page, start, …) in time order: the new log, the
    // upload's rows and each book's totals.
    storage.write(&t, &[])?;
    let mut time_order = Vec::new();
    let mut per_book: Vec<(u64, Vec<u8>)> = vec![(0, Vec::new()); books.len()];
    let mut failed = None;
    extsort::each(storage, &m, STAT, |r| {
        let [start, book, page, duration, pages] = fields(r).map(|v| v as u32);
        time_order.extend_from_slice(&stat_record([book, page, start, duration, pages]));
        if let Ok(at) = books.binary_search_by_key(&(book as u16), |b| b.id) {
            let total = &mut per_book[at];
            total.0 += u64::from(duration);
            let now = if books[at].pages > 0 {
                books[at].pages
            } else {
                pages
            };
            for p in totals::rescaled(page, pages, now) {
                let (byte, bit) = ((p / 8) as usize, p % 8);
                if total.1.len() <= byte {
                    total.1.resize(byte + 1, 0);
                }
                total.1[byte] |= 1 << bit;
            }
        }
        if time_order.len() >= 4096 {
            if let Err(e) = storage.append(&t, &time_order) {
                failed = Some(e);
            }
            time_order.clear();
        }
    })?;
    storage.append(&t, &time_order)?;
    if let Some(e) = failed {
        return Err(Error::Storage(e));
    }
    // The new log replaces the old one.
    store::replace_log(storage, &t)?;
    store::save_books(storage, &books);

    if let Some(out) = out {
        let theirs = |md5: &str| {
            income
                .as_ref()
                .and_then(|s| s.books.iter().find(|b| b.md5 == md5))
        };
        let ko_books: Vec<KoBook> = books
            .iter()
            .zip(per_book.iter())
            .map(|(b, (seconds, read))| KoBook {
                id: i64::from(b.id),
                title: b.title.clone(),
                authors: b.authors.clone(),
                notes: theirs(&b.md5).map_or(0, |t| t.notes),
                last_open: b.last_open,
                highlights: theirs(&b.md5).map_or(0, |t| t.highlights),
                pages: i64::from(b.pages),
                series: na(&b.series),
                language: na(&b.language),
                md5: b.md5.clone(),
                total_read_time: *seconds as i64,
                total_read_pages: read.iter().map(|x| x.count_ones()).sum::<u32>() as i64,
            })
            .collect();
        koreader::write(storage, out, &ko_books, &t, &path("merge-write"))?;
    }
    for file in [&l, &i, &c, &m, &t] {
        let _ = storage.remove(file);
    }
    Ok(Outcome {
        books: books.len(),
        stays,
    })
}

/// KOReader writes `N/A` for what a book does not say.
fn na(text: &str) -> String {
    if text.is_empty() {
        String::from("N/A")
    } else {
        text.to_string()
    }
}

/// A sorted file of stays, read ahead by one.
struct Source<'s> {
    records: extsort::Records<'s>,
    head: Option<Vec<u8>>,
}

impl<'s> Source<'s> {
    fn new(storage: &'s dyn Storage, path: &str) -> Self {
        let mut records = extsort::Records::new(storage, path, STAT);
        let head = records.next();
        Self { records, head }
    }

    fn peek(&self) -> Option<&Vec<u8>> {
        self.head.as_ref()
    }

    fn next(&mut self) -> Option<Vec<u8>> {
        let head = self.head.take();
        self.head = self.records.next();
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PageStat;
    use quireink_sdk::MemoryStorage;

    const DIR: &str = ".quireink/plugins/stats";

    fn book(id: u16, md5: &str) -> Book {
        Book {
            id,
            title: format!("Ours {id}"),
            authors: String::from("N/A"),
            md5: md5.to_string(),
            pages: 100,
            language: String::from("de"),
            series: String::from("N/A"),
            last_open: 10,
            path: format!("books/{id}.epub"),
        }
    }

    fn stay(book: u16, page: u32, start: u32, duration: u16) -> PageStat {
        PageStat {
            book,
            page,
            start,
            duration,
            pages: 100,
        }
    }

    fn log(storage: &MemoryStorage) -> Vec<(u16, u32, u32, u16)> {
        let mut out = Vec::new();
        store::scan(storage, |s| out.push((s.book, s.page, s.start, s.duration)));
        out
    }

    /// A KOReader file from another device: its own ids and names.
    fn other_device(
        storage: &MemoryStorage,
        path: &str,
        books: &[(i64, &str, &str)],
        stays: &[(i64, u32, u32, u32)],
    ) {
        let ko: Vec<KoBook> = books
            .iter()
            .map(|(id, title, md5)| KoBook {
                id: *id,
                title: title.to_string(),
                authors: String::from("Autorin"),
                pages: 200,
                md5: md5.to_string(),
                series: String::from("N/A"),
                language: String::from("de"),
                last_open: 99,
                ..KoBook::default()
            })
            .collect();
        let mut records = Vec::new();
        let mut sorted = stays.to_vec();
        sorted.sort_by_key(|s| s.2);
        for (book, page, start, duration) in sorted {
            records.extend_from_slice(&stat_record([book as u32, page, start, duration, 200]));
        }
        storage.write("stays.tmp", &records).unwrap();
        koreader::write(storage, path, &ko, "stays.tmp", "tmp").unwrap();
    }

    #[test]
    fn merges_like_koreader() {
        let storage = MemoryStorage::new();
        store::save_books(&storage, &[book(1, "aaa"), book(2, "bbb")]);
        store::append(&storage, stay(1, 5, 1000, 30));
        store::append(&storage, stay(1, 6, 1040, 20));
        store::append(&storage, stay(2, 1, 1100, 10));
        // The other device: book "aaa" as its id 7 (a stay the same, one
        // longer, one new), and a book unknown here.
        other_device(
            &storage,
            "income.db",
            &[(7, "Their title", "aaa"), (8, "Neu", "ccc")],
            &[
                (7, 5, 1000, 25),
                (7, 6, 1040, 50),
                (7, 9, 2000, 40),
                (8, 3, 3000, 60),
            ],
        );
        let outcome = merge(&storage, Some("income.db"), None, Some("out.db"), DIR).unwrap();
        assert_eq!(outcome, Outcome { books: 3, stays: 5 });
        let books = store::books(&storage);
        assert_eq!(
            books.iter().map(|b| b.md5.as_str()).collect::<Vec<_>>(),
            ["aaa", "bbb", "ccc"]
        );
        // Named as KOReader names them, the path here kept.
        assert_eq!(
            (books[0].title.as_str(), books[0].authors.as_str()),
            ("Their title", "Autorin")
        );
        assert_eq!(books[0].path, "books/1.epub");
        assert_eq!(books[2].id, 3);
        // In time order; the longer duration of a stay wins.
        assert_eq!(
            log(&storage),
            [
                (1, 5, 1000, 30),
                (1, 6, 1040, 50),
                (2, 1, 1100, 10),
                (1, 9, 2000, 40),
                (3, 3, 3000, 60)
            ]
        );
        // The file to upload reads back with the same.
        let db = Database::open(&storage, "out.db").unwrap();
        assert_eq!(db.user_version, koreader::SCHEMA_VERSION);
        let ko = koreader::books(&db).unwrap();
        assert_eq!(ko.len(), 3);
        assert_eq!(ko[0].total_read_time, 120);
        let mut stays = 0;
        koreader::stats(&db, |_| stays += 1).unwrap();
        assert_eq!(stays, 5);
    }

    #[test]
    fn deletions_go_both_ways() {
        let storage = MemoryStorage::new();
        store::save_books(&storage, &[book(1, "aaa"), book(2, "bbb")]);
        store::append(&storage, stay(1, 1, 100, 10));
        store::append(&storage, stay(1, 2, 200, 10));
        store::append(&storage, stay(2, 1, 300, 10));
        // The last upload had both books and three stays, plus a stay of
        // book 1 that was since deleted here.
        other_device(
            &storage,
            "cached.db",
            &[(1, "Ours 1", "aaa"), (2, "Ours 2", "bbb")],
            &[
                (1, 1, 100, 10),
                (1, 2, 200, 10),
                (1, 3, 250, 10),
                (2, 1, 300, 10),
            ],
        );
        // There, book "bbb" and the stay at 200 were deleted since; the
        // stay at 250 (deleted here) is still there.
        other_device(
            &storage,
            "income.db",
            &[(4, "Ours 1", "aaa")],
            &[(4, 1, 100, 10), (4, 3, 250, 10)],
        );
        let outcome = merge(&storage, Some("income.db"), Some("cached.db"), None, DIR).unwrap();
        assert_eq!(outcome, Outcome { books: 1, stays: 1 });
        assert_eq!(log(&storage), [(1, 1, 100, 10)]);
        assert_eq!(store::books(&storage).len(), 1);
    }

    /// Merges a real KOReader file into made-up statistics and writes the
    /// result for SQLite to check:
    /// `QUIREINK_KO_IN=… QUIREINK_KO_OUT=… cargo test -p quireink-plugin-stats -- --ignored`.
    #[test]
    #[ignore]
    fn real_koreader_file() {
        extern crate std;
        let (Ok(input), Ok(output)) = (
            std::env::var("QUIREINK_KO_IN"),
            std::env::var("QUIREINK_KO_OUT"),
        ) else {
            return;
        };
        let storage = MemoryStorage::new().with("income.db", std::fs::read(input).unwrap());
        store::save_books(&storage, &[book(1, "made-up-md5")]);
        store::append(&storage, stay(1, 1, 1_700_000_000, 30));
        let outcome = merge(&storage, Some("income.db"), None, Some("out.db"), DIR).unwrap();
        std::println!("{outcome:?}");
        std::fs::write(output, storage.get("out.db").unwrap()).unwrap();
    }
}

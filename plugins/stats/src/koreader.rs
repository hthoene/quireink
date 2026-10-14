//! KOReader's statistics database (`statistics.sqlite3`, schema version
//! 20221111): reading its books and page stays, and writing the same kind
//! of file from QuireInk's statistics, so KOReader can merge it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::sqlite::{
    Database, Error, SchemaEntry, Value, Writer, compare_records, encode_record,
};

use crate::extsort;

/// The schema version KOReader writes (`PRAGMA user_version`).
pub(crate) const SCHEMA_VERSION: u32 = 20_221_111;

/// A row of KOReader's `book` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct KoBook {
    pub id: i64,
    pub title: String,
    pub authors: String,
    pub notes: i64,
    pub last_open: i64,
    pub highlights: i64,
    pub pages: i64,
    pub series: String,
    pub language: String,
    pub md5: String,
    pub total_read_time: i64,
    pub total_read_pages: i64,
}

/// A row of `page_stat_data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KoStat {
    pub book: i64,
    pub page: i64,
    pub start: i64,
    pub duration: i64,
    pub pages: i64,
}

/// The books of a statistics database.
pub(crate) fn books(db: &Database<'_>) -> Result<Vec<KoBook>, Error> {
    let (columns, root) = db.table("book")?;
    let at = |name: &str| columns.iter().position(|c| c.eq_ignore_ascii_case(name));
    let id_column = at("id");
    let mut books = Vec::new();
    db.scan(root, &mut |rowid, values| {
        let get = |name: &str| at(name).and_then(|i| values.get(i));
        let text = |name: &str| {
            get(name)
                .and_then(Value::as_text)
                .unwrap_or_default()
                .to_string()
        };
        let int = |name: &str| get(name).and_then(Value::as_int).unwrap_or(0);
        // `id` is the row id (an INTEGER PRIMARY KEY).
        let id = id_column
            .and_then(|i| values.get(i))
            .and_then(Value::as_int)
            .unwrap_or(rowid);
        books.push(KoBook {
            id,
            title: text("title"),
            authors: text("authors"),
            notes: int("notes"),
            last_open: int("last_open"),
            highlights: int("highlights"),
            pages: int("pages"),
            series: text("series"),
            language: text("language"),
            md5: text("md5"),
            total_read_time: int("total_read_time"),
            total_read_pages: int("total_read_pages"),
        });
        true
    })?;
    Ok(books)
}

/// Calls `visit` with every page stay.
pub(crate) fn stats(db: &Database<'_>, mut visit: impl FnMut(KoStat)) -> Result<(), Error> {
    let (columns, root) = db.table("page_stat_data")?;
    let at = |name: &str| columns.iter().position(|c| c.eq_ignore_ascii_case(name));
    let (book, page, start, duration, pages) = (
        at("id_book"),
        at("page"),
        at("start_time"),
        at("duration"),
        at("total_pages"),
    );
    db.scan(root, &mut |_, values| {
        let int = |i: Option<usize>| {
            i.and_then(|i| values.get(i))
                .and_then(Value::as_int)
                .unwrap_or(0)
        };
        visit(KoStat {
            book: int(book),
            page: int(page),
            start: int(start),
            duration: int(duration),
            pages: int(pages),
        });
        true
    })
}

const BOOK_SQL: &str = "CREATE TABLE book
            (
                id integer PRIMARY KEY autoincrement,
                title text,
                authors text,
                notes      integer,
                last_open  integer,
                highlights integer,
                pages      integer,
                series text,
                language text,
                md5 text,
                total_read_time  integer,
                total_read_pages integer
            )";

const PAGE_STAT_DATA_SQL: &str = "CREATE TABLE page_stat_data
        (
            id_book     integer,
            page        integer NOT NULL DEFAULT 0,
            start_time  integer NOT NULL DEFAULT 0,
            duration    integer NOT NULL DEFAULT 0,
            total_pages integer NOT NULL DEFAULT 0,
            UNIQUE (id_book, page, start_time),
            FOREIGN KEY(id_book) REFERENCES book(id)
        )";

const NUMBERS_SQL: &str = "CREATE TABLE numbers
        (
            number INTEGER PRIMARY KEY
        )";

const PAGE_STAT_SQL: &str = "CREATE VIEW page_stat AS
        SELECT id_book, first_page + idx - 1 AS page, start_time, duration / (last_page - first_page + 1) AS duration
        FROM (
            SELECT id_book, page, total_pages, pages, start_time, duration,
                -- First page_number for this page after rescaling single row
                ((page - 1) * pages) / total_pages + 1 AS first_page,
                -- Last page_number for this page after rescaling single row
                max(((page - 1) * pages) / total_pages + 1, (page * pages) / total_pages) AS last_page,
                idx
            FROM page_stat_data
            JOIN book ON book.id = id_book
            -- Duplicate rows for multiple pages as needed (as a result of rescaling)
            JOIN (SELECT number as idx FROM numbers) AS N ON idx <= (last_page - first_page + 1)
        )";

/// Bytes of a stay in the files `write` reads: book, page, start,
/// duration and pages, each a big-endian u32, sorted by start.
pub(crate) const STAT: usize = 20;

/// The five numbers of a [`STAT`] record.
pub(crate) fn fields(r: &[u8]) -> [i64; 5] {
    let n = |i: usize| i64::from(u32::from_be_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]));
    [n(0), n(4), n(8), n(12), n(16)]
}

/// A [`STAT`] record: book, page, start, duration and pages.
pub(crate) fn stat_record(values: [u32; 5]) -> [u8; STAT] {
    let mut out = [0u8; STAT];
    for (i, v) in values.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Writes a statistics database to `path`: `books` (with ids) and the
/// stays in `stats` (a file of [`STAT`] records sorted by start time,
/// which become rows 1, 2, …). `temp` names working files.
pub(crate) fn write(
    storage: &dyn Storage,
    path: &str,
    books: &[KoBook],
    stats: &str,
    temp: &str,
) -> Result<(), Error> {
    let mut writer = Writer::new(storage, path);
    let int = Value::Int;
    let text = |t: &str| Value::Text(t.to_string());
    let book_rows: Vec<(i64, Vec<u8>)> = books
        .iter()
        .map(|b| {
            let record = encode_record(&[
                // The id is the row id.
                Value::Null,
                text(&b.title),
                text(&b.authors),
                int(b.notes),
                int(b.last_open),
                int(b.highlights),
                int(b.pages),
                text(&b.series),
                text(&b.language),
                text(&b.md5),
                int(b.total_read_time),
                int(b.total_read_pages),
            ]);
            (b.id, record)
        })
        .collect();
    let book_root = writer.table(book_rows)?;
    let last_id = books.iter().map(|b| b.id).max().unwrap_or(0);
    let sequence = writer.table([(1, encode_record(&[text("book"), int(last_id)]))])?;
    let mut book_index: Vec<Vec<u8>> = books
        .iter()
        .map(|b| encode_record(&[text(&b.title), text(&b.authors), text(&b.md5), int(b.id)]))
        .collect();
    book_index.sort_by(|a, b| compare_records(a, b));
    let book_index_root = writer.index(book_index)?;

    // The stays, row by row in time order; the keys of the unique index
    // (book, page, start, row) go to a file to be sorted on the way.
    let keys = alloc::format!("{temp}.keys");
    storage.write(&keys, &[])?;
    let pending = core::cell::RefCell::new(Vec::<u8>::new());
    let failed = core::cell::Cell::new(None);
    let flush = |force: bool| {
        let mut pending = pending.borrow_mut();
        if (force || pending.len() >= 4096) && !pending.is_empty() {
            if let Err(e) = storage.append(&keys, &pending) {
                failed.set(Some(e));
            }
            pending.clear();
        }
    };
    let mut records = extsort::Records::new(storage, stats, STAT);
    let rows = records.by_ref().enumerate().map(|(i, r)| {
        let rowid = i as i64 + 1;
        let [book, page, start, duration, pages] = fields(&r);
        for v in [book, page, start, rowid] {
            pending
                .borrow_mut()
                .extend_from_slice(&(v as u32).to_be_bytes());
        }
        flush(false);
        (
            rowid,
            encode_record(&[int(book), int(page), int(start), int(duration), int(pages)]),
        )
    });
    let stat_root = writer.table(rows)?;
    flush(true);
    if let Some(e) = records.failed.take().or(failed.take()) {
        return Err(Error::Storage(e));
    }
    extsort::sort(storage, &keys, &keys, 16, temp)?;
    let unique = extsort::Records::new(storage, &keys, 16).map(|r| {
        let n = |i: usize| i64::from(u32::from_be_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]));
        encode_record(&[int(n(0)), int(n(4)), int(n(8)), int(n(12))])
    });
    let unique_root = writer.index(unique)?;
    let _ = storage.remove(&keys);
    // By start time: the rows are in that order already.
    let by_start = extsort::Records::new(storage, stats, STAT)
        .enumerate()
        .map(|(i, r)| encode_record(&[int(fields(&r)[2]), int(i as i64 + 1)]));
    let start_root = writer.index(by_start)?;
    let numbers_root = writer.table((1..=1000).map(|n| (n, encode_record(&[Value::Null]))))?;

    let entry = |kind: &str, name: &str, table: &str, root: u32, sql: &str| SchemaEntry {
        kind: kind.to_string(),
        name: name.to_string(),
        table: table.to_string(),
        root,
        sql: sql.to_string(),
    };
    let schema = [
        entry("table", "book", "book", book_root, BOOK_SQL),
        entry(
            "table",
            "sqlite_sequence",
            "sqlite_sequence",
            sequence,
            "CREATE TABLE sqlite_sequence(name,seq)",
        ),
        entry(
            "index",
            "book_title_authors_md5",
            "book",
            book_index_root,
            "CREATE UNIQUE INDEX book_title_authors_md5 ON book(title, authors, md5)",
        ),
        entry(
            "table",
            "page_stat_data",
            "page_stat_data",
            stat_root,
            PAGE_STAT_DATA_SQL,
        ),
        entry(
            "index",
            "sqlite_autoindex_page_stat_data_1",
            "page_stat_data",
            unique_root,
            "",
        ),
        entry(
            "index",
            "page_stat_data_start_time",
            "page_stat_data",
            start_root,
            "CREATE INDEX page_stat_data_start_time ON page_stat_data(start_time)",
        ),
        entry("table", "numbers", "numbers", numbers_root, NUMBERS_SQL),
        entry("view", "page_stat", "page_stat", 0, PAGE_STAT_SQL),
    ];
    writer.finish(&schema, SCHEMA_VERSION)
}

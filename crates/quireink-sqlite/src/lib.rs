//! Reading and writing SQLite database files, as far as exchanging data
//! needs it: e.g. KOReader's reading statistics.
//!
//! - [`Database`] reads tables page by page from storage, so memory stays
//!   at a page or two whatever the file's size. Rollback-journal and WAL
//!   files (after a checkpoint) both work; free pages are ignored.
//! - [`Writer`] builds a new database file from rows and index entries
//!   given in order: tables and indexes as B-trees written leaf by leaf,
//!   long values in overflow pages, the schema on page 1.
//!
//! Not supported: changing a file in place, WITHOUT ROWID tables, UTF-16
//! databases. `no_std`, no dependencies but storage.

#![no_std]

extern crate alloc;

mod read;
mod record;
mod varint;
mod write;

pub use read::{Database, SchemaEntry, columns};
pub use record::{Value, compare_records, decode_record, encode_record};
pub use write::Writer;

use core::fmt;

use quireink_platform::StorageError;

/// What can go wrong with a database file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not an SQLite database (or an unsupported kind: UTF-16).
    NotSqlite,
    /// The file contradicts itself.
    Corrupt(&'static str),
    /// A table or index the caller asked for is not there.
    Missing,
    /// The schema does not fit on the first page.
    SchemaTooLarge,
    Storage(StorageError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotSqlite => f.write_str("not an SQLite database"),
            Error::Corrupt(what) => write!(f, "corrupt database: {what}"),
            Error::Missing => f.write_str("no such table"),
            Error::SchemaTooLarge => f.write_str("schema too large"),
            Error::Storage(e) => write!(f, "{e}"),
        }
    }
}

impl From<StorageError> for Error {
    fn from(error: StorageError) -> Self {
        Error::Storage(error)
    }
}

/// Page types.
const TABLE_INTERIOR: u8 = 0x05;
const TABLE_LEAF: u8 = 0x0d;
const INDEX_INTERIOR: u8 = 0x02;
const INDEX_LEAF: u8 = 0x0a;

/// How much of a payload of `size` bytes stays in its cell, with `usable`
/// bytes per page; the rest goes to overflow pages.
fn local_size(size: usize, usable: usize, table_leaf: bool) -> usize {
    let max = if table_leaf {
        usable - 35
    } else {
        (usable - 12) * 64 / 255 - 23
    };
    if size <= max {
        return size;
    }
    let min = (usable - 12) * 32 / 255 - 23;
    let k = min + (size - min) % (usable - 4);
    if k <= max { k } else { min }
}

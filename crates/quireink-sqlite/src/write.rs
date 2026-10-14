//! Writing a new database file.
//!
//! Pages are appended to `<path>.part` as trees are built: leaves first,
//! then the levels above them. [`Writer::finish`] writes page 1 (header
//! and schema) to `<path>` and copies the rest after it, so a reader never
//! sees a half-written database.
//!
//! Cells are written only when their page is: payloads too long for a
//! cell go to overflow pages then, so no page is ever left unused.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use quireink_platform::Storage;

use crate::record::{Value, encode_record};
use crate::{
    Error, INDEX_INTERIOR, INDEX_LEAF, SchemaEntry, TABLE_INTERIOR, TABLE_LEAF, local_size, varint,
};

const PAGE: usize = 4096;
/// SQLite 3.45.0, the format this writer follows.
const SQLITE_VERSION: u32 = 3_045_000;

/// Builds a database file.
pub struct Writer<'s> {
    storage: &'s dyn Storage,
    path: String,
    part: String,
    /// The next page's number.
    next: u32,
    failed: Option<Error>,
}

impl core::fmt::Debug for Writer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Writer")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// A cell before it is written: what precedes the payload (child page,
/// sizes, row id) and the payload.
struct Cell {
    prefix: Vec<u8>,
    payload: Vec<u8>,
    table_leaf: bool,
}

impl Cell {
    /// Its size in the page, with the link to overflow pages if any.
    fn len(&self) -> usize {
        let local = local_size(self.payload.len(), PAGE, self.table_leaf);
        let overflow = if local < self.payload.len() { 4 } else { 0 };
        self.prefix.len() + local + overflow
    }
}

fn header_len(kind: u8) -> usize {
    if kind == TABLE_INTERIOR || kind == INDEX_INTERIOR {
        12
    } else {
        8
    }
}

/// Whether cells of `lens` fit on a page of `kind` whose header starts at
/// `start`.
fn fits(kind: u8, start: usize, lens: impl Iterator<Item = usize>) -> bool {
    let used: usize = lens.map(|l| l + 2).sum();
    start + header_len(kind) + used <= PAGE
}

/// How cells `0..n` split into the nodes of an interior level: each node
/// holds cells `s..e` and child `e` as its right-most pointer; the next
/// one starts at `e + 1`. Every node keeps at least one cell.
fn plan(kind: u8, lens: &[usize]) -> Vec<(usize, usize)> {
    let n = lens.len();
    let mut nodes = Vec::new();
    let mut start = 0;
    let mut used = 0;
    let room = PAGE - header_len(kind);
    let mut i = 0;
    while i < n {
        if used + lens[i] + 2 <= room || i == start {
            used += lens[i] + 2;
            i += 1;
            continue;
        }
        // Full: cell `i` is the boundary, unless that left the last node
        // without a cell; then the one before is.
        let end = if i + 1 == n && i - 1 > start {
            i - 1
        } else {
            i
        };
        nodes.push((start, end));
        start = end + 1;
        i = start;
        used = 0;
    }
    nodes.push((start, n));
    nodes
}

impl<'s> Writer<'s> {
    pub fn new(storage: &'s dyn Storage, path: &str) -> Self {
        let part = format!("{path}.part");
        let failed = storage.write(&part, &[]).err().map(Error::Storage);
        Self {
            storage,
            path: path.to_string(),
            part,
            // Page 1 comes last.
            next: 2,
            failed,
        }
    }

    /// Appends a page; returns its number.
    fn append(&mut self, page: &[u8]) -> u32 {
        if self.failed.is_none()
            && let Err(e) = self.storage.append(&self.part, page)
        {
            self.failed = Some(Error::Storage(e));
        }
        self.next += 1;
        self.next - 1
    }

    /// A cell's bytes; what does not fit goes to overflow pages written
    /// now, linked from the cell's last four bytes.
    fn write_cell(&mut self, cell: &Cell) -> Vec<u8> {
        let mut out = cell.prefix.clone();
        let local = local_size(cell.payload.len(), PAGE, cell.table_leaf);
        out.extend_from_slice(&cell.payload[..local]);
        if local < cell.payload.len() {
            let chunks: Vec<&[u8]> = cell.payload[local..].chunks(PAGE - 4).collect();
            let first = self.next;
            for (i, chunk) in chunks.iter().enumerate() {
                let mut page = vec![0u8; PAGE];
                let next = if i + 1 < chunks.len() {
                    first + i as u32 + 1
                } else {
                    0
                };
                page[..4].copy_from_slice(&next.to_be_bytes());
                page[4..4 + chunk.len()].copy_from_slice(chunk);
                self.append(&page);
            }
            out.extend_from_slice(&first.to_be_bytes());
        }
        out
    }

    /// A page of `kind` with `cells`; `right` is an interior page's
    /// right-most child; `start` where the header begins (100 on page 1).
    fn page_bytes(&mut self, kind: u8, start: usize, cells: &[Cell], right: u32) -> Vec<u8> {
        let mut page = vec![0u8; PAGE];
        page[start] = kind;
        page[start + 3..start + 5].copy_from_slice(&(cells.len() as u16).to_be_bytes());
        let mut content = PAGE;
        let mut pointer = start + header_len(kind);
        for cell in cells {
            let bytes = self.write_cell(cell);
            content -= bytes.len();
            page[content..content + bytes.len()].copy_from_slice(&bytes);
            page[pointer..pointer + 2].copy_from_slice(&(content as u16).to_be_bytes());
            pointer += 2;
        }
        page[start + 5..start + 7].copy_from_slice(&(content as u16).to_be_bytes());
        if header_len(kind) == 12 {
            page[start + 8..start + 12].copy_from_slice(&right.to_be_bytes());
        }
        page
    }

    fn write_page(&mut self, kind: u8, cells: &[Cell], right: u32) -> u32 {
        let page = self.page_bytes(kind, 0, cells, right);
        self.append(&page)
    }

    /// Writes a table of `(rowid, record)` rows, given in order of their
    /// ids; returns its root page.
    pub fn table(&mut self, rows: impl IntoIterator<Item = (i64, Vec<u8>)>) -> Result<u32, Error> {
        // Leaves: (page, largest id in it).
        let mut children: Vec<(u32, i64)> = Vec::new();
        let mut leaf: Vec<Cell> = Vec::new();
        let mut last = 0;
        for (rowid, record) in rows {
            let mut prefix = Vec::new();
            varint::write(&mut prefix, record.len() as u64);
            varint::write(&mut prefix, rowid as u64);
            let cell = Cell {
                prefix,
                payload: record,
                table_leaf: true,
            };
            let lens = leaf.iter().map(Cell::len).chain([cell.len()]);
            if !leaf.is_empty() && !fits(TABLE_LEAF, 0, lens) {
                let page = self.write_page(TABLE_LEAF, &leaf, 0);
                children.push((page, last));
                leaf.clear();
            }
            leaf.push(cell);
            last = rowid;
        }
        let page = self.write_page(TABLE_LEAF, &leaf, 0);
        children.push((page, last));
        // Levels above: a cell per child (its page and largest id) but the
        // right-most.
        while children.len() > 1 {
            let n = children.len() - 1;
            let cells: Vec<Cell> = children[..n]
                .iter()
                .map(|(page, key)| {
                    let mut prefix = page.to_be_bytes().to_vec();
                    varint::write(&mut prefix, *key as u64);
                    Cell {
                        prefix,
                        payload: Vec::new(),
                        table_leaf: false,
                    }
                })
                .collect();
            let lens: Vec<usize> = cells.iter().map(Cell::len).collect();
            let mut parents = Vec::new();
            for (start, end) in plan(TABLE_INTERIOR, &lens) {
                let (right, key) = children[end];
                let page = self.write_page(TABLE_INTERIOR, &cells[start..end], right);
                parents.push((page, key));
            }
            children = parents;
        }
        self.failed.clone().map_or(Ok(children[0].0), Err)
    }

    /// Writes an index of `entries` (records whose last value is the row
    /// id), given in order; returns its root page.
    pub fn index(&mut self, entries: impl IntoIterator<Item = Vec<u8>>) -> Result<u32, Error> {
        let cell = |entry: Vec<u8>| {
            let mut prefix = Vec::new();
            varint::write(&mut prefix, entry.len() as u64);
            Cell {
                prefix,
                payload: entry,
                table_leaf: false,
            }
        };
        // Leaves and the entries between them: an index keeps each entry
        // once, so the entry after a full leaf goes up as the separator.
        let mut children: Vec<u32> = Vec::new();
        let mut separators: Vec<Vec<u8>> = Vec::new();
        let mut leaf: Vec<Cell> = Vec::new();
        let mut entries = entries.into_iter().peekable();
        while let Some(entry) = entries.next() {
            let c = cell(entry);
            let lens = leaf.iter().map(Cell::len).chain([c.len()]);
            if leaf.is_empty() || fits(INDEX_LEAF, 0, lens) {
                leaf.push(c);
                continue;
            }
            // Full. The entry after it separates this leaf from the next;
            // if there is none, the leaf's own last entry does, so the
            // last leaf is never empty.
            let separator = if entries.peek().is_some() {
                c.payload
            } else {
                let last = leaf.pop().map(|l| l.payload).unwrap_or_default();
                let page = self.write_page(INDEX_LEAF, &leaf, 0);
                children.push(page);
                separators.push(last);
                leaf = Vec::from([c]);
                continue;
            };
            let page = self.write_page(INDEX_LEAF, &leaf, 0);
            children.push(page);
            separators.push(separator);
            leaf.clear();
        }
        let page = self.write_page(INDEX_LEAF, &leaf, 0);
        children.push(page);
        while children.len() > 1 {
            let cells: Vec<Cell> = separators
                .iter()
                .zip(children.iter())
                .map(|(separator, child)| {
                    let mut prefix = child.to_be_bytes().to_vec();
                    varint::write(&mut prefix, separator.len() as u64);
                    Cell {
                        prefix,
                        payload: separator.clone(),
                        table_leaf: false,
                    }
                })
                .collect();
            let lens: Vec<usize> = cells.iter().map(Cell::len).collect();
            let mut up_children = Vec::new();
            let mut up_separators = Vec::new();
            for (start, end) in plan(INDEX_INTERIOR, &lens) {
                let page = self.write_page(INDEX_INTERIOR, &cells[start..end], children[end]);
                up_children.push(page);
                if end < separators.len() {
                    up_separators.push(separators[end].clone());
                }
            }
            children = up_children;
            separators = up_separators;
        }
        self.failed.clone().map_or(Ok(children[0]), Err)
    }

    /// Writes the file: page 1 with the header and the `schema`, then the
    /// pages built. `user_version` is the application's schema version.
    pub fn finish(mut self, schema: &[SchemaEntry], user_version: u32) -> Result<(), Error> {
        if let Some(e) = self.failed.take() {
            let _ = self.storage.remove(&self.part);
            return Err(e);
        }
        let mut cells = Vec::new();
        for (i, entry) in schema.iter().enumerate() {
            let sql = if entry.sql.is_empty() {
                Value::Null
            } else {
                Value::Text(entry.sql.clone())
            };
            let record = encode_record(&[
                Value::Text(entry.kind.clone()),
                Value::Text(entry.name.clone()),
                Value::Text(entry.table.clone()),
                Value::Int(i64::from(entry.root)),
                sql,
            ]);
            if local_size(record.len(), PAGE, true) < record.len() {
                return Err(Error::SchemaTooLarge);
            }
            let mut prefix = Vec::new();
            varint::write(&mut prefix, record.len() as u64);
            varint::write(&mut prefix, i as u64 + 1);
            cells.push(Cell {
                prefix,
                payload: record,
                table_leaf: true,
            });
        }
        if !fits(TABLE_LEAF, 100, cells.iter().map(Cell::len)) {
            return Err(Error::SchemaTooLarge);
        }
        let mut first = self.page_bytes(TABLE_LEAF, 100, &cells, 0);
        let pages = self.next - 1;
        let header = &mut first[..100];
        header[..16].copy_from_slice(b"SQLite format 3\0");
        header[16..18].copy_from_slice(&(PAGE as u16).to_be_bytes());
        // File format versions (rollback journal), no reserved bytes,
        // payload fractions 64, 32, 32.
        header[18..24].copy_from_slice(&[1, 1, 0, 64, 32, 32]);
        let be = |v: u32| v.to_be_bytes();
        header[24..28].copy_from_slice(&be(1)); // change counter
        header[28..32].copy_from_slice(&be(pages));
        header[40..44].copy_from_slice(&be(1)); // schema cookie
        header[44..48].copy_from_slice(&be(4)); // schema format
        header[56..60].copy_from_slice(&be(1)); // UTF-8
        header[60..64].copy_from_slice(&be(user_version));
        header[92..96].copy_from_slice(&be(1)); // version valid for
        header[96..100].copy_from_slice(&be(SQLITE_VERSION));
        self.storage.write(&self.path, &first)?;
        let mut chunk = vec![0u8; PAGE * 8];
        let mut at = 0u64;
        loop {
            let n = self.storage.read_at(&self.part, at, &mut chunk)?;
            if n == 0 {
                break;
            }
            self.storage.append(&self.path, &chunk[..n])?;
            at += n as u64;
        }
        let _ = self.storage.remove(&self.part);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Database, compare_records, decode_record};
    use alloc::string::ToString;
    use quireink_platform::Storage;

    /// The in-memory storage of the SDK is not reachable from here.
    #[derive(Default)]
    pub(super) struct Memory(core::cell::RefCell<Vec<(String, Vec<u8>)>>);

    impl Storage for Memory {
        fn read_at(
            &self,
            path: &str,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<usize, quireink_platform::StorageError> {
            let files = self.0.borrow();
            let (_, data) = files
                .iter()
                .find(|(p, _)| p == path)
                .ok_or(quireink_platform::StorageError::NotFound)?;
            let start = (offset as usize).min(data.len());
            let n = buf.len().min(data.len() - start);
            buf[..n].copy_from_slice(&data[start..start + n]);
            Ok(n)
        }
        fn size(&self, path: &str) -> Result<u64, quireink_platform::StorageError> {
            let files = self.0.borrow();
            files
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, d)| d.len() as u64)
                .ok_or(quireink_platform::StorageError::NotFound)
        }
        fn write(&self, path: &str, data: &[u8]) -> Result<(), quireink_platform::StorageError> {
            let mut files = self.0.borrow_mut();
            files.retain(|(p, _)| p != path);
            files.push((path.to_string(), data.to_vec()));
            Ok(())
        }
        fn append(&self, path: &str, data: &[u8]) -> Result<(), quireink_platform::StorageError> {
            let mut files = self.0.borrow_mut();
            match files.iter_mut().find(|(p, _)| p == path) {
                Some((_, d)) => d.extend_from_slice(data),
                None => files.push((path.to_string(), data.to_vec())),
            }
            Ok(())
        }
        fn list(
            &self,
            _: &str,
            _: &mut dyn FnMut(quireink_platform::DirEntry<'_>),
        ) -> Result<(), quireink_platform::StorageError> {
            Ok(())
        }
        fn remove(&self, path: &str) -> Result<(), quireink_platform::StorageError> {
            self.0.borrow_mut().retain(|(p, _)| p != path);
            Ok(())
        }
    }

    #[test]
    fn plans_keep_a_cell_in_every_node() {
        // 1000 bytes a cell: four fit in a page.
        let lens = vec![1000; 9];
        let nodes = plan(INDEX_INTERIOR, &lens);
        assert!(nodes.iter().all(|(s, e)| e > s), "{nodes:?}");
        // Cells and boundaries cover every cell once.
        let covered: usize = nodes.iter().map(|(s, e)| e - s).sum::<usize>() + nodes.len() - 1;
        assert_eq!(covered, 9);
        assert_eq!(nodes.last().map(|n| n.1), Some(9));
    }

    #[test]
    fn tables_and_indexes_read_back() {
        let storage = Memory::default();
        let mut writer = Writer::new(&storage, "db");
        let long = "x".repeat(9000);
        let rows: Vec<(i64, Vec<u8>)> = (1..=3000)
            .map(|i| {
                let text = if i == 1500 {
                    long.clone()
                } else {
                    alloc::format!("row {i}")
                };
                (i, encode_record(&[Value::Int(i * 7), Value::Text(text)]))
            })
            .collect();
        let table = writer.table(rows.clone()).unwrap();
        let mut entries: Vec<Vec<u8>> = (1..=3000i64)
            .map(|i| encode_record(&[Value::Int((i * 37) % 1000), Value::Int(i)]))
            .collect();
        entries.sort_by(|a, b| compare_records(a, b));
        let index = writer.index(entries).unwrap();
        let schema = [
            SchemaEntry {
                kind: "table".to_string(),
                name: "t".to_string(),
                table: "t".to_string(),
                root: table,
                sql: "CREATE TABLE t(a integer, b text)".to_string(),
            },
            SchemaEntry {
                kind: "index".to_string(),
                name: "t_a".to_string(),
                table: "t".to_string(),
                root: index,
                sql: "CREATE INDEX t_a ON t(a)".to_string(),
            },
        ];
        writer.finish(&schema, 7).unwrap();
        let db = Database::open(&storage, "db").unwrap();
        assert_eq!(db.user_version, 7);
        assert_eq!(db.schema().unwrap(), schema);
        let (columns, root) = db.table("t").unwrap();
        assert_eq!(columns, ["a", "b"]);
        let mut read = Vec::new();
        db.scan(root, &mut |id, values| {
            read.push((id, encode_record(values)));
            true
        })
        .unwrap();
        assert_eq!(read, rows);
        assert!(decode_record(&read[1499].1).is_some());
        assert!(storage.size("db.part").is_err(), "the part file is gone");
    }
}

/// Writes a sample database for SQLite itself to check:
/// `QUIREINK_SQLITE_OUT=x.db cargo test -p quireink-sqlite -- --ignored`.
#[cfg(test)]
mod check {
    extern crate std;

    use super::*;
    use crate::{compare_records, encode_record};
    use alloc::string::ToString;

    #[test]
    #[ignore]
    fn sample_for_sqlite() {
        let Ok(out) = std::env::var("QUIREINK_SQLITE_OUT") else {
            return;
        };
        let storage = tests_storage();
        let mut writer = Writer::new(&storage, "db");
        let long = "y".repeat(12_000);
        let rows: Vec<(i64, Vec<u8>)> = (1..=20_000)
            .map(|i| {
                let text = if i % 5000 == 0 {
                    long.clone()
                } else {
                    alloc::format!("row {i}")
                };
                (i, encode_record(&[Value::Int(i % 977), Value::Text(text)]))
            })
            .collect();
        let table = writer.table(rows).unwrap();
        let mut entries: Vec<Vec<u8>> = (1..=20_000i64)
            .map(|i| {
                let text = if i % 5000 == 0 {
                    "y".repeat(12_000)
                } else {
                    alloc::format!("row {i}")
                };
                encode_record(&[Value::Int(i % 977), Value::Text(text), Value::Int(i)])
            })
            .collect();
        entries.sort_by(|a, b| compare_records(a, b));
        let index = writer.index(entries).unwrap();
        let schema = [
            SchemaEntry {
                kind: "table".to_string(),
                name: "t".to_string(),
                table: "t".to_string(),
                root: table,
                sql: "CREATE TABLE t(a integer, b text)".to_string(),
            },
            SchemaEntry {
                kind: "index".to_string(),
                name: "t_ab".to_string(),
                table: "t".to_string(),
                root: index,
                sql: "CREATE INDEX t_ab ON t(a, b)".to_string(),
            },
        ];
        writer.finish(&schema, 20_221_111).unwrap();
        let mut data = vec![0u8; storage.size("db").unwrap() as usize];
        storage.read_at("db", 0, &mut data).unwrap();
        std::fs::write(out, data).unwrap();
    }

    fn tests_storage() -> super::tests::Memory {
        super::tests::Memory::default()
    }
}

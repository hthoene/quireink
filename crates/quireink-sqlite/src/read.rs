//! Reading tables from a database file.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use quireink_platform::Storage;

use crate::record::{Value, decode_record};
use crate::{Error, TABLE_INTERIOR, TABLE_LEAF, local_size, varint};

/// Deepest B-tree followed, against loops in corrupt files.
const MAX_DEPTH: u32 = 20;

/// A row of `sqlite_schema`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaEntry {
    /// `table`, `index`, `view` or `trigger`.
    pub kind: String,
    pub name: String,
    pub table: String,
    pub root: u32,
    pub sql: String,
}

/// An open database file.
pub struct Database<'s> {
    storage: &'s dyn Storage,
    path: String,
    page_size: usize,
    usable: usize,
    pub user_version: u32,
}

impl core::fmt::Debug for Database<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Database")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl<'s> Database<'s> {
    pub fn open(storage: &'s dyn Storage, path: &str) -> Result<Self, Error> {
        let mut header = [0u8; 100];
        if storage.read_at(path, 0, &mut header)? != 100 || &header[..16] != b"SQLite format 3\0" {
            return Err(Error::NotSqlite);
        }
        let page_size = match u16::from_be_bytes([header[16], header[17]]) {
            1 => 65_536,
            n if n >= 512 && n.is_power_of_two() => usize::from(n),
            _ => return Err(Error::Corrupt("page size")),
        };
        // UTF-8 only.
        if u32::from_be_bytes([header[56], header[57], header[58], header[59]]) > 1 {
            return Err(Error::NotSqlite);
        }
        Ok(Self {
            storage,
            path: path.to_string(),
            page_size,
            usable: page_size - usize::from(header[20]),
            user_version: u32::from_be_bytes([header[60], header[61], header[62], header[63]]),
        })
    }

    fn page(&self, number: u32) -> Result<Vec<u8>, Error> {
        if number == 0 {
            return Err(Error::Corrupt("page 0"));
        }
        let mut page = vec![0u8; self.page_size];
        let offset = u64::from(number - 1) * self.page_size as u64;
        if self.storage.read_at(&self.path, offset, &mut page)? != self.page_size {
            return Err(Error::Corrupt("short page"));
        }
        Ok(page)
    }

    /// The tables, indexes, views and triggers.
    pub fn schema(&self) -> Result<Vec<SchemaEntry>, Error> {
        let mut entries = Vec::new();
        self.scan(1, &mut |_, values| {
            let text = |i: usize| {
                values
                    .get(i)
                    .and_then(Value::as_text)
                    .unwrap_or_default()
                    .to_string()
            };
            entries.push(SchemaEntry {
                kind: text(0),
                name: text(1),
                table: text(2),
                root: values.get(3).and_then(Value::as_int).unwrap_or(0) as u32,
                sql: text(4),
            });
            true
        })?;
        Ok(entries)
    }

    /// The column names and root page of table `name`.
    pub fn table(&self, name: &str) -> Result<(Vec<String>, u32), Error> {
        let entry = self
            .schema()?
            .into_iter()
            .find(|e| e.kind == "table" && e.name.eq_ignore_ascii_case(name))
            .ok_or(Error::Missing)?;
        Ok((columns(&entry.sql), entry.root))
    }

    /// Calls `visit` with every row's id and values of the table at page
    /// `root`, in order of their ids, until it returns `false`.
    pub fn scan(
        &self,
        root: u32,
        visit: &mut dyn FnMut(i64, &[Value]) -> bool,
    ) -> Result<(), Error> {
        self.walk(root, 0, visit).map(|_| ())
    }

    /// Visits a subtree; `Ok(false)` once `visit` asked to stop.
    fn walk(
        &self,
        number: u32,
        depth: u32,
        visit: &mut dyn FnMut(i64, &[Value]) -> bool,
    ) -> Result<bool, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::Corrupt("too deep"));
        }
        let page = self.page(number)?;
        let at = if number == 1 { 100 } else { 0 };
        let kind = page[at];
        let cells = usize::from(u16::from_be_bytes([page[at + 3], page[at + 4]]));
        let header = if kind == TABLE_INTERIOR { 12 } else { 8 };
        let pointer = |i: usize| -> Result<usize, Error> {
            let p = at + header + i * 2;
            let offset = page
                .get(p..p + 2)
                .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
                .ok_or(Error::Corrupt("cell pointer"))?;
            if offset >= page.len() {
                return Err(Error::Corrupt("cell offset"));
            }
            Ok(offset)
        };
        match kind {
            TABLE_INTERIOR => {
                for i in 0..cells {
                    let cell = pointer(i)?;
                    let child = u32::from_be_bytes([
                        page[cell],
                        page[cell + 1],
                        page[cell + 2],
                        page[cell + 3],
                    ]);
                    if !self.walk(child, depth + 1, visit)? {
                        return Ok(false);
                    }
                }
                let right =
                    u32::from_be_bytes([page[at + 8], page[at + 9], page[at + 10], page[at + 11]]);
                self.walk(right, depth + 1, visit)
            }
            TABLE_LEAF => {
                for i in 0..cells {
                    let cell = pointer(i)?;
                    let (size, n) = varint::read(&page[cell..]).ok_or(Error::Corrupt("cell"))?;
                    let (rowid, m) =
                        varint::read(&page[cell + n..]).ok_or(Error::Corrupt("cell"))?;
                    let payload = self.payload(&page, cell + n + m, size as usize)?;
                    let values = decode_record(&payload).ok_or(Error::Corrupt("record"))?;
                    if !visit(rowid as i64, &values) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            _ => Err(Error::Corrupt("not a table page")),
        }
    }

    /// A table leaf cell's payload of `size` bytes starting at `start`,
    /// with what overflows from following pages.
    fn payload(&self, page: &[u8], start: usize, size: usize) -> Result<Vec<u8>, Error> {
        let local = local_size(size, self.usable, true);
        let mut payload = page
            .get(start..start + local)
            .ok_or(Error::Corrupt("payload"))?
            .to_vec();
        if local < size {
            let at = start + local;
            let mut next = page
                .get(at..at + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or(Error::Corrupt("overflow"))?;
            let mut hops = 0;
            while payload.len() < size {
                hops += 1;
                if next == 0 || hops > 1_000_000 {
                    return Err(Error::Corrupt("overflow chain"));
                }
                let overflow = self.page(next)?;
                next = u32::from_be_bytes([overflow[0], overflow[1], overflow[2], overflow[3]]);
                let take = (size - payload.len()).min(self.usable - 4);
                payload.extend_from_slice(&overflow[4..4 + take]);
            }
        }
        Ok(payload)
    }
}

/// The column names of a `CREATE TABLE` statement, in order.
pub fn columns(sql: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (sql.find('('), sql.rfind(')')) else {
        return Vec::new();
    };
    let body = &sql[open + 1..close.max(open + 1)];
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (i, c) in body.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    parts
        .into_iter()
        .filter_map(|part| {
            // Lines may hold comments.
            let part: String = part
                .lines()
                .map(|l| l.split("--").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join(" ");
            let name = part
                .split_whitespace()
                .next()?
                .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']');
            let constraint = ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"]
                .iter()
                .any(|k| name.eq_ignore_ascii_case(k));
            (!constraint && !name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_come_from_create_table() {
        let sql = "CREATE TABLE page_stat_data\n (\n id_book integer, -- the book\n page integer NOT NULL DEFAULT 0,\n start_time integer,\n UNIQUE (id_book, page, start_time),\n FOREIGN KEY(id_book) REFERENCES book(id)\n )";
        assert_eq!(columns(sql), ["id_book", "page", "start_time"]);
        assert_eq!(
            columns("CREATE TABLE \"t\"(\"a\" text, [b], c DEFAULT (1+2))"),
            ["a", "b", "c"]
        );
    }
}

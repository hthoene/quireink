//! Lookups in `.idx` and `.syn` files through a block index.
//!
//! Both files are sorted lists of variable-length entries: a word, a NUL
//! byte and a fixed-size tail (definition offset and size, or the number
//! of a headword). The block index stores the byte offset of every 32nd
//! entry, so a lookup binary-searches the blocks by their first word and
//! then reads at most a block and a bit of entries.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

use quireink_platform::Storage;

use crate::{Error, ascii_casecmp, malformed};

/// Entries per block.
const BLOCK: u32 = 32;
/// Block index file: magic, source size (u64), entry count (u32), then
/// one little-endian u32 offset per block.
const MAGIC: &[u8; 4] = b"QBI1";
const HEADER: u64 = 16;
/// Longest word in an entry (StarDict's limit is 256 bytes).
const MAX_WORD: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `.idx`: definition offset (u32 or u64) and size (u32), big-endian.
    Words { tail: usize },
    /// `.syn`: the number of a headword (u32, big-endian).
    Synonyms,
}

impl Kind {
    fn tail(self) -> usize {
        match self {
            Kind::Words { tail } => tail,
            Kind::Synonyms => 4,
        }
    }
}

/// A headword and where its definition is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Headword {
    pub word: String,
    pub offset: u64,
    pub size: u32,
}

#[derive(Debug)]
pub(crate) struct Index {
    path: String,
    cache: String,
    size: u64,
    count: u32,
    kind: Kind,
}

/// An entry as read from the file.
struct Entry {
    word: Vec<u8>,
    tail: [u8; 12],
    next: u64,
}

impl Index {
    pub fn new(
        storage: &dyn Storage,
        path: String,
        cache: String,
        count: u32,
        kind: Kind,
    ) -> Result<Index, Error> {
        let size = storage.size(&path)?;
        Ok(Index {
            path,
            cache,
            size,
            count,
            kind,
        })
    }

    fn blocks(&self) -> u32 {
        self.count.div_ceil(BLOCK)
    }

    fn header(&self) -> [u8; HEADER as usize] {
        let mut header = [0u8; HEADER as usize];
        header[..4].copy_from_slice(MAGIC);
        header[4..12].copy_from_slice(&self.size.to_le_bytes());
        header[12..16].copy_from_slice(&self.count.to_le_bytes());
        header
    }

    /// Whether the block index exists and belongs to this file.
    pub fn is_built(&self, storage: &dyn Storage) -> bool {
        let expected = HEADER + 4 * u64::from(self.blocks());
        let mut header = [0u8; HEADER as usize];
        storage.size(&self.cache) == Ok(expected)
            && storage.read_at(&self.cache, 0, &mut header) == Ok(header.len())
            && header == self.header()
    }

    pub fn builder(&self) -> Builder {
        Builder {
            source: self.path.clone(),
            target: self.cache.clone(),
            header: self.header(),
            tail: self.kind.tail(),
            size: self.size,
            count: self.count,
            pos: 0,
            entry: 0,
            started: false,
            pending: Vec::new(),
        }
    }

    fn block_offset(&self, storage: &dyn Storage, block: u32) -> Result<u64, Error> {
        let mut buf = [0u8; 4];
        let at = HEADER + 4 * u64::from(block);
        if storage.read_at(&self.cache, at, &mut buf)? != 4 {
            return Err(Error::NotPrepared);
        }
        Ok(u64::from(u32::from_le_bytes(buf)))
    }

    fn entry(&self, storage: &dyn Storage, offset: u64) -> Result<Entry, Error> {
        let tail = self.kind.tail();
        let mut buf = [0u8; MAX_WORD + 1 + 12];
        let n = storage.read_at(&self.path, offset, &mut buf[..MAX_WORD + 1 + tail])?;
        let end = buf[..n]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| malformed("index entry without end"))?;
        if end + 1 + tail > n {
            return Err(malformed("index entry cut short"));
        }
        let mut tail_bytes = [0u8; 12];
        tail_bytes[..tail].copy_from_slice(&buf[end + 1..end + 1 + tail]);
        Ok(Entry {
            word: buf[..end].to_vec(),
            tail: tail_bytes,
            next: offset + (end + 1 + tail) as u64,
        })
    }

    /// Calls `visit` for each entry whose word equals `key` without ASCII
    /// case, up to `max` of them.
    fn scan(
        &self,
        storage: &dyn Storage,
        key: &[u8],
        max: usize,
        visit: &mut dyn FnMut(Entry),
    ) -> Result<(), Error> {
        if !self.is_built(storage) {
            return Err(Error::NotPrepared);
        }
        // The first block whose first word is not before `key`; matches
        // can start in the block before it.
        let (mut lo, mut hi) = (0, self.blocks());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let first = self.entry(storage, self.block_offset(storage, mid)?)?;
            if ascii_casecmp(&first.word, key) == Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let block = lo.saturating_sub(1);
        let mut number = block * BLOCK;
        let mut offset = self.block_offset(storage, block)?;
        let mut found = 0;
        while number < self.count && offset < self.size {
            let entry = self.entry(storage, offset)?;
            offset = entry.next;
            number += 1;
            match ascii_casecmp(&entry.word, key) {
                Ordering::Less => {}
                Ordering::Equal => {
                    visit(entry);
                    found += 1;
                    if found >= max {
                        break;
                    }
                }
                Ordering::Greater => break,
            }
        }
        Ok(())
    }

    /// Headwords of an `.idx` equal to `key` (ignoring ASCII case).
    pub fn headwords(
        &self,
        storage: &dyn Storage,
        key: &str,
        max: usize,
    ) -> Result<Vec<Headword>, Error> {
        let mut found = Vec::new();
        let kind = self.kind;
        self.scan(storage, key.as_bytes(), max, &mut |entry| {
            found.push(headword(kind, entry));
        })?;
        Ok(found)
    }

    /// Headword numbers a `.syn` gives for `key`.
    pub fn synonyms(
        &self,
        storage: &dyn Storage,
        key: &str,
        max: usize,
    ) -> Result<Vec<u32>, Error> {
        let mut found = Vec::new();
        self.scan(storage, key.as_bytes(), max, &mut |entry| {
            let t = entry.tail;
            found.push(u32::from_be_bytes([t[0], t[1], t[2], t[3]]));
        })?;
        Ok(found)
    }

    /// Headword number `number` of an `.idx`.
    pub fn headword(&self, storage: &dyn Storage, number: u32) -> Result<Headword, Error> {
        if number >= self.count {
            return Err(malformed("synonym points past the index"));
        }
        let mut offset = self.block_offset(storage, number / BLOCK)?;
        for _ in 0..number % BLOCK {
            offset = self.entry(storage, offset)?.next;
        }
        Ok(headword(self.kind, self.entry(storage, offset)?))
    }
}

fn headword(kind: Kind, entry: Entry) -> Headword {
    let t = entry.tail;
    let (offset, size) = if kind == (Kind::Words { tail: 12 }) {
        (
            u64::from_be_bytes([t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7]]),
            u32::from_be_bytes([t[8], t[9], t[10], t[11]]),
        )
    } else {
        (
            u64::from(u32::from_be_bytes([t[0], t[1], t[2], t[3]])),
            u32::from_be_bytes([t[4], t[5], t[6], t[7]]),
        )
    };
    Headword {
        word: String::from_utf8_lossy(&entry.word).into_owned(),
        offset,
        size,
    }
}

/// Builds a block index a step at a time, so a large dictionary can be
/// prepared in the background.
#[derive(Debug)]
pub struct Builder {
    source: String,
    target: String,
    header: [u8; HEADER as usize],
    tail: usize,
    size: u64,
    count: u32,
    pos: u64,
    entry: u32,
    started: bool,
    /// Offsets not written yet.
    pending: Vec<u8>,
}

impl Builder {
    /// How far it is: `(bytes read, bytes in total)`.
    pub fn progress(&self) -> (u64, u64) {
        (self.pos, self.size)
    }

    /// Reads about `budget` bytes of the file; `true` once the block index
    /// is complete.
    pub fn step(&mut self, storage: &dyn Storage, budget: usize) -> Result<bool, Error> {
        if !self.started {
            storage.write(&self.target, &self.header)?;
            self.started = true;
        }
        let mut buf = vec![0u8; 4096];
        let mut read = 0;
        while read < budget && self.entry < self.count {
            let n = storage.read_at(&self.source, self.pos, &mut buf)?;
            if n == 0 {
                return Err(malformed("index ends early"));
            }
            let mut p = 0;
            while self.entry < self.count {
                let Some(zero) = buf[p..n].iter().position(|&b| b == 0) else {
                    break;
                };
                let end = p + zero + 1 + self.tail;
                if end > n {
                    break;
                }
                if self.entry % BLOCK == 0 {
                    let offset = u32::try_from(self.pos + p as u64)
                        .map_err(|_| malformed("index larger than 4 GiB"))?;
                    self.pending.extend_from_slice(&offset.to_le_bytes());
                }
                self.entry += 1;
                p = end;
            }
            if p == 0 {
                return Err(malformed("index entry too long"));
            }
            self.pos += p as u64;
            read += p;
            if self.pending.len() >= 1024 {
                storage.append(&self.target, &self.pending)?;
                self.pending.clear();
            }
        }
        if !self.pending.is_empty() {
            storage.append(&self.target, &self.pending)?;
            self.pending.clear();
        }
        Ok(self.entry == self.count)
    }
}

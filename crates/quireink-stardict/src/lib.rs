//! StarDict dictionaries, looked up in place with little memory.
//!
//! A StarDict dictionary is a set of files next to each other:
//!
//! - `name.ifo`: the description (word count, how definitions are typed);
//! - `name.idx`: the sorted headwords, each with the offset and size of
//!   its definition;
//! - `name.dict` or `name.dict.dz` (dictzip: gzip in independently
//!   compressed chunks): the definitions;
//! - `name.syn` (optional): more words, e.g. inflected forms, each pointing
//!   to a headword.
//!
//! Nothing is loaded into memory as a whole. A small *block index* (the
//! offset of every 32nd entry, 4 bytes each) is built once per file with
//! [`Builder`] and stored next to the plugin's other data; lookups then
//! binary-search the blocks and read a few hundred bytes. Reading a
//! definition from a dictzip file inflates one chunk as a stream through a
//! 32 KiB window.

#![no_std]

extern crate alloc;

mod dict;
mod index;
mod text;

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use quireink_platform::{Storage, StorageError};

pub use index::Builder;
pub use text::to_plain_text;

use dict::DictFile;
use index::{Index, Kind};

/// Why a dictionary cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    Storage(StorageError),
    /// A file is missing or not in the expected format.
    Malformed(Cow<'static, str>),
    /// The block index has not been built yet (see [`Dictionary::builders`]).
    NotPrepared,
    OutOfMemory,
}

impl From<StorageError> for Error {
    fn from(error: StorageError) -> Self {
        Error::Storage(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Storage(e) => write!(f, "storage: {e:?}"),
            Error::Malformed(message) => write!(f, "malformed dictionary: {message}"),
            Error::NotPrepared => f.write_str("dictionary index not built yet"),
            Error::OutOfMemory => f.write_str("out of memory"),
        }
    }
}

pub(crate) fn malformed(message: &'static str) -> Error {
    Error::Malformed(Cow::Borrowed(message))
}

/// What the `.ifo` file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The dictionary's name.
    pub name: String,
    pub word_count: u32,
    pub syn_count: u32,
    /// 32 or 64: the size of definition offsets in the `.idx` file.
    pub offset_bits: u8,
    /// The type of every definition field, if all entries share them
    /// (`sametypesequence`), e.g. `m` for plain text, `h` for HTML.
    pub types: Option<String>,
}

impl Info {
    /// Parses an `.ifo` file.
    pub fn parse(text: &str) -> Result<Info, Error> {
        let mut lines = text.lines();
        if !lines.next().is_some_and(|first| {
            first
                .trim_start_matches('\u{feff}')
                .starts_with("StarDict's dict ifo file")
        }) {
            return Err(malformed("not a StarDict .ifo file"));
        }
        let mut info = Info {
            name: String::new(),
            word_count: 0,
            syn_count: 0,
            offset_bits: 32,
            types: None,
        };
        for line in lines {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "bookname" => info.name = value.to_string(),
                "wordcount" => info.word_count = value.parse().unwrap_or(0),
                "synwordcount" => info.syn_count = value.parse().unwrap_or(0),
                "idxoffsetbits" => info.offset_bits = if value == "64" { 64 } else { 32 },
                "sametypesequence" if !value.is_empty() => info.types = Some(value.to_string()),
                _ => {}
            }
        }
        if info.word_count == 0 {
            return Err(malformed("no words"));
        }
        Ok(info)
    }
}

/// A definition found for a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// The headword, as written in the dictionary.
    pub word: String,
    /// The definition as plain text: paragraphs separated by newlines.
    pub text: String,
}

/// An opened dictionary.
#[derive(Debug)]
pub struct Dictionary {
    pub info: Info,
    idx: Index,
    syn: Option<Index>,
    dict: DictFile,
}

/// Longest definition read (larger ones are cut).
const MAX_DEFINITION: usize = 64 * 1024;
/// Most definitions returned for one word.
const MAX_RESULTS: usize = 8;

impl Dictionary {
    /// Opens the dictionary described by `ifo` (a path ending in `.ifo`).
    /// Its block indexes are kept in `cache` (a folder).
    pub fn open(storage: &dyn Storage, ifo: &str, cache: &str) -> Result<Dictionary, Error> {
        let base = ifo
            .strip_suffix(".ifo")
            .ok_or_else(|| malformed("not an .ifo path"))?;
        let data = read_small(storage, ifo, 16 * 1024)?;
        let info = Info::parse(&String::from_utf8_lossy(&data))?;
        let idx_tail = if info.offset_bits == 64 { 12 } else { 8 };
        let idx = Index::new(
            storage,
            format!("{base}.idx"),
            format!("{cache}/idx.qbi"),
            info.word_count,
            Kind::Words { tail: idx_tail },
        )?;
        let syn = (info.syn_count > 0)
            .then(|| {
                Index::new(
                    storage,
                    format!("{base}.syn"),
                    format!("{cache}/syn.qbi"),
                    info.syn_count,
                    Kind::Synonyms,
                )
            })
            .transpose()
            .unwrap_or(None);
        let dict = DictFile::open(storage, base)?;
        Ok(Dictionary {
            info,
            idx,
            syn,
            dict,
        })
    }

    /// Builders for the block indexes that are missing; run them (e.g. a
    /// step at a time in the background) before looking up words.
    pub fn builders(&self, storage: &dyn Storage) -> Vec<Builder> {
        [Some(&self.idx), self.syn.as_ref()]
            .into_iter()
            .flatten()
            .filter(|index| !index.is_built(storage))
            .map(Index::builder)
            .collect()
    }

    /// The definitions of `word`: exact matches ignoring ASCII case, also
    /// through synonyms (e.g. inflected forms); if there are none, the
    /// same for the word in lower case.
    pub fn lookup(&self, storage: &dyn Storage, word: &str) -> Result<Vec<Definition>, Error> {
        let word = word.trim();
        if word.is_empty() {
            return Ok(Vec::new());
        }
        let mut found = self.find(storage, word)?;
        let lower = word.to_lowercase();
        if found.is_empty() && lower != word {
            found = self.find(storage, &lower)?;
        }
        let mut definitions = Vec::new();
        for entry in found.into_iter().take(MAX_RESULTS) {
            let size = (entry.size as usize).min(MAX_DEFINITION);
            let data = self.dict.read(storage, entry.offset, size)?;
            definitions.push(Definition {
                text: text::decode(&data, self.info.types.as_deref()),
                word: entry.word,
            });
        }
        Ok(definitions)
    }

    /// Headwords matching `word`, directly or through synonyms, without
    /// duplicates.
    fn find(&self, storage: &dyn Storage, word: &str) -> Result<Vec<index::Headword>, Error> {
        let mut found = self.idx.headwords(storage, word, MAX_RESULTS)?;
        if let Some(syn) = &self.syn {
            for number in syn.synonyms(storage, word, MAX_RESULTS)? {
                if found.len() >= MAX_RESULTS {
                    break;
                }
                let headword = self.idx.headword(storage, number)?;
                if !found.iter().any(|h| h.offset == headword.offset) {
                    found.push(headword);
                }
            }
        }
        Ok(found)
    }
}

/// Reads a whole small file.
pub(crate) fn read_small(storage: &dyn Storage, path: &str, max: usize) -> Result<Vec<u8>, Error> {
    let size = storage.size(path)? as usize;
    if size > max {
        return Err(malformed("file too large"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| Error::OutOfMemory)?;
    data.resize(size, 0);
    let mut read = 0;
    while read < size {
        let n = storage.read_at(path, read as u64, &mut data[read..])?;
        if n == 0 {
            return Err(Error::Storage(StorageError::Io));
        }
        read += n;
    }
    Ok(data)
}

/// StarDict sorts by ASCII letters without case, then by exact bytes;
/// lookups match the first part.
pub(crate) fn ascii_casecmp(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    a.iter()
        .map(u8::to_ascii_lowercase)
        .cmp(b.iter().map(u8::to_ascii_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ifo() {
        let info = Info::parse(
            "StarDict's dict ifo file\nversion=2.4.2\nbookname=Test – Wörter\nwordcount=3\nsynwordcount=2\nsametypesequence=h\n",
        )
        .unwrap();
        assert_eq!(info.name, "Test – Wörter");
        assert_eq!((info.word_count, info.syn_count), (3, 2));
        assert_eq!(info.types.as_deref(), Some("h"));
        assert!(Info::parse("hello").is_err());
    }

    #[test]
    fn compares_without_ascii_case() {
        use core::cmp::Ordering::*;
        assert_eq!(ascii_casecmp(b"Abc", b"abd"), Less);
        assert_eq!(ascii_casecmp(b"Hus", b"hus"), Equal);
        // Other letters keep their case: StarDict compares bytes.
        assert_ne!(ascii_casecmp("Ø".as_bytes(), "ø".as_bytes()), Equal);
    }
}

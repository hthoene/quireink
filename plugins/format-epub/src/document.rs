use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::document::{
    Block, Document, DocumentError, Locator, Metadata, OffsetMap, TocEntry,
};
use quireink_sdk::zip::ZipArchive;
use quireink_sdk::{Env, Storage, StorageError};

use crate::PLUGIN;
use crate::convert::{self, CachePaths, HEADER};
use crate::package::{self, RawToc};

/// Bump when the conversion changes, so caches are rebuilt.
const CACHE_VERSION: u32 = 2;

/// An open EPUB. Chapters are converted on first use and read from the
/// cache afterwards.
#[derive(Debug)]
pub struct EpubDocument {
    archive: ZipArchive,
    /// Archive paths of the spine documents.
    spine: Vec<String>,
    /// Uncompressed sizes of the spine documents, for progress estimates.
    sizes: Vec<u32>,
    metadata: Metadata,
    toc: Vec<RawToc>,
    cache: String,
    /// Length of each converted chapter's text, once known.
    lengths: RefCell<Vec<Option<u32>>>,
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn u32_at(storage: &dyn Storage, path: &str, offset: u64) -> Result<u32, StorageError> {
    let mut buf = [0u8; 4];
    if storage.read_at(path, offset, &mut buf)? != 4 {
        return Err(StorageError::Io);
    }
    Ok(u32::from_le_bytes(buf))
}

impl EpubDocument {
    pub fn open(storage: &dyn Storage, path: &str) -> Result<Self, DocumentError> {
        let archive = ZipArchive::open(storage, path).map_err(package::error)?;
        let package = package::read(storage, &archive)?;
        let sizes = package
            .spine
            .iter()
            .map(|p| archive.entry(p).map_or(0, |e| e.size))
            .collect();
        let mut metadata = package.metadata;
        if metadata.title.is_none() {
            let name = path.rsplit('/').next().unwrap_or(path);
            metadata.title = Some(String::from(name.rsplit_once('.').map_or(name, |(s, _)| s)));
        }
        let cache = alloc::format!("{}/{:016x}", Env::plugin_dir(PLUGIN.as_str()), fnv(path));
        let document = EpubDocument {
            lengths: RefCell::new(alloc::vec![None; package.spine.len()]),
            spine: package.spine,
            sizes,
            metadata,
            toc: package.toc,
            archive,
            cache,
        };
        document.validate_cache(storage, storage.size(path)?);
        Ok(document)
    }

    /// Drops cached chapters made from another file or converter version.
    fn validate_cache(&self, storage: &dyn Storage, size: u64) {
        let meta = alloc::format!("{}/meta.ini", self.cache);
        let expected = alloc::format!("[cache]\nversion = {CACHE_VERSION}\nsize = {size}\n");
        let current = quireink_sdk::read_file(storage, &meta, 1024).unwrap_or_default();
        if current != expected.as_bytes() {
            for section in 0..self.spine.len() {
                let _ = storage.remove(&self.paths(section as u32).end);
            }
            let _ = storage.write(&meta, expected.as_bytes());
        }
    }

    fn paths(&self, section: u32) -> CachePaths {
        CachePaths {
            text: alloc::format!("{}/{section}.txt", self.cache),
            index: alloc::format!("{}/{section}.idx", self.cache),
            anchors: alloc::format!("{}/{section}.anc", self.cache),
            end: alloc::format!("{}/{section}.end", self.cache),
        }
    }

    /// Converts a chapter if needed; returns its paths, text length and
    /// number of blocks.
    fn chapter(
        &self,
        storage: &dyn Storage,
        section: u32,
    ) -> Result<(CachePaths, u32, u32), DocumentError> {
        let paths = self.paths(section);
        let length = match u32_at(storage, &paths.end, 0) {
            Ok(length) => length,
            Err(_) => {
                let name = &self.spine[section as usize];
                let entry = self.archive.entry(name).ok_or_else(|| {
                    DocumentError::Malformed(Cow::Owned(alloc::format!("missing {name}")))
                })?;
                convert::convert(storage, &self.archive, entry, &paths)?
            }
        };
        if let Some(slot) = self.lengths.borrow_mut().get_mut(section as usize) {
            *slot = Some(length);
        }
        let blocks = (storage.size(&paths.index)? / 4) as u32;
        Ok((paths, length, blocks))
    }

    /// Reads block `i` of a chapter.
    fn read_block(
        &self,
        storage: &dyn Storage,
        paths: &CachePaths,
        length: u32,
        blocks: u32,
        i: u32,
    ) -> Result<Block, DocumentError> {
        let start = u32_at(storage, &paths.index, u64::from(i) * 4)?;
        let end = if i + 1 < blocks {
            u32_at(storage, &paths.index, u64::from(i + 1) * 4)?
        } else {
            length
        };
        let mut header = [0u8; HEADER as usize];
        storage.read_at(&paths.text, u64::from(start - HEADER), &mut header)?;
        let len = usize::from(u16::from_le_bytes([header[4], header[5]]));
        let mut text = alloc::vec![0u8; len];
        let mut filled = 0;
        while filled < len {
            let n = storage.read_at(
                &paths.text,
                u64::from(start) + filled as u64,
                &mut text[filled..],
            )?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        text.truncate(filled);
        Ok(Block {
            kind: convert::decode_kind(header[0], header[1]),
            continues: convert::continues(header[2]),
            start,
            end,
            text: String::from_utf8_lossy(&text).into_owned(),
            map: OffsetMap::IDENTITY,
        })
    }

    /// The offset of anchor `id` in a chapter, scanning the anchor file
    /// line by line.
    fn anchor(&self, storage: &dyn Storage, section: u32, id: &str) -> Option<u32> {
        let (paths, ..) = self.chapter(storage, section).ok()?;
        let mut chunk = [0u8; 256];
        let mut line: Vec<u8> = Vec::new();
        let mut offset = 0u64;
        loop {
            let n = storage.read_at(&paths.anchors, offset, &mut chunk).ok()?;
            offset += n as u64;
            for &byte in &chunk[..n] {
                if byte != b'\n' {
                    // Longer lines cannot match a sensible id.
                    if line.len() < 512 {
                        line.push(byte);
                    }
                    continue;
                }
                if let Some(tab) = line.iter().position(|&b| b == b'\t')
                    && &line[..tab] == id.as_bytes()
                {
                    return core::str::from_utf8(&line[tab + 1..]).ok()?.parse().ok();
                }
                line.clear();
            }
            if n == 0 {
                return None;
            }
        }
    }
}

impl Document for EpubDocument {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn sections(&self) -> u32 {
        self.spine.len() as u32
    }

    fn section_len(&self, section: u32) -> u32 {
        self.lengths
            .borrow()
            .get(section as usize)
            .copied()
            .flatten()
            .unwrap_or_else(|| self.sizes.get(section as usize).copied().unwrap_or(0))
    }

    /// Estimated from the chapters' sizes in the archive, so it needs no
    /// conversion of unread chapters.
    fn progress(&self, at: Locator) -> f32 {
        let total: u64 = self.sizes.iter().map(|&s| u64::from(s)).sum();
        if total == 0 {
            return 0.0;
        }
        let section = (at.section as usize).min(self.sizes.len());
        let before: u64 = self.sizes[..section].iter().map(|&s| u64::from(s)).sum();
        let within = match (
            self.sizes.get(section),
            self.lengths.borrow().get(section).copied().flatten(),
        ) {
            (Some(&size), Some(length)) if length > 0 => {
                u64::from(size) * u64::from(at.offset.min(length)) / u64::from(length)
            }
            _ => 0,
        };
        ((before + within) as f32 / total as f32).min(1.0)
    }

    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError> {
        if at.section as usize >= self.spine.len() {
            return Ok(None);
        }
        let (paths, length, blocks) = self.chapter(storage, at.section)?;
        if blocks == 0 || at.offset >= length {
            return Ok(None);
        }
        // The last block whose text starts at or before the offset.
        let (mut low, mut high) = (0u32, blocks);
        while low < high {
            let mid = low + (high - low) / 2;
            if u32_at(storage, &paths.index, u64::from(mid) * 4)? <= at.offset {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let i = low.saturating_sub(1);
        self.read_block(storage, &paths, length, blocks, i)
            .map(Some)
    }

    fn toc(&self, storage: &dyn Storage) -> Vec<TocEntry> {
        self.toc
            .iter()
            .filter_map(|entry| {
                let section = self.spine.iter().position(|p| *p == entry.file)? as u32;
                let offset = entry
                    .fragment
                    .as_deref()
                    .and_then(|id| self.anchor(storage, section, id))
                    .unwrap_or(0);
                Some(TocEntry {
                    title: entry.title.clone(),
                    level: entry.level,
                    at: Locator::new(section, offset),
                })
            })
            .collect()
    }
}

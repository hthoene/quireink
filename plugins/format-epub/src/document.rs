use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::document::{
    Align, Block, BlockKind, Document, DocumentError, EmbeddedFont, ImageRef, Locator, Metadata,
    OffsetMap, Run, TextStyle, TocEntry,
};
use quireink_sdk::image::GrayFile;
use quireink_sdk::zip::ZipArchive;
use quireink_sdk::{Env, Storage, StorageError};

use crate::PLUGIN;
use crate::convert::{self, CachePaths, HEADER, INDEX};
use crate::obfuscation;
use crate::package::{self, Chapter};

/// Largest embedded font read.
const MAX_FONT: u64 = 16 * 1024 * 1024;

/// Marks a chapter whose converted length is not known yet.
const UNKNOWN: u32 = u32::MAX;

/// An open EPUB. The package is read once into the cache; chapters are
/// converted on first use and read from the cache afterwards. In memory:
/// 8 bytes per archive entry and 12 per chapter.
#[derive(Debug)]
pub struct EpubDocument {
    archive: ZipArchive,
    /// The spine: archive record and uncompressed size per chapter.
    spine: Vec<Chapter>,
    /// Length of each converted chapter's text, once known.
    lengths: Vec<Cell<u32>>,
    metadata: Metadata,
    toc_len: u32,
    cache: String,
    /// Where converted chapters are: `cache`, or its `plain` folder when
    /// the book's styles are off.
    chapters: String,
    /// Whether the book's CSS applies.
    styles: bool,
    /// Archive path of the cover image.
    cover: Option<String>,
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn paths(cache: &str, section: u32) -> CachePaths {
    CachePaths {
        dir: String::from(cache),
        text: alloc::format!("{cache}/{section}.txt"),
        index: alloc::format!("{cache}/{section}.idx"),
        anchors: alloc::format!("{cache}/{section}.anc"),
        positions: alloc::format!("{cache}/{section}.pos"),
        end: alloc::format!("{cache}/{section}.end"),
    }
}

/// The section and the element path below `<body>` of an XPointer such
/// as `/body/DocFragment[3]/body/div/p[5]/text().12`: indexes are filled in
/// (`div` is `div[1]`), text positions dropped.
fn parse_xpointer(xpointer: &str) -> Option<(u32, String)> {
    let rest = xpointer.trim().strip_prefix("/body/DocFragment")?;
    let (index, rest) = match rest.strip_prefix('[') {
        Some(r) => {
            let (n, r) = r.split_once(']')?;
            (n.parse::<u32>().ok()?, r)
        }
        None => (1, rest),
    };
    let rest = rest.strip_prefix("/body").unwrap_or(rest);
    let mut segments: Vec<String> = Vec::new();
    for segment in rest.split('/').filter(|s| !s.is_empty()) {
        if segment.starts_with("text()") {
            break;
        }
        // A position inside an element (`p[3].5`) counts as the element.
        let segment = segment.split('.').next().unwrap_or(segment);
        if segment.contains('[') {
            segments.push(String::from(segment));
        } else {
            segments.push(alloc::format!("{segment}[1]"));
        }
    }
    Some((index.checked_sub(1)?, segments.join("/")))
}

/// Where chapters converted without the book's styles are kept.
fn plain_cache(cache: &str) -> String {
    alloc::format!("{cache}/plain")
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
        Self::open_with(storage, path, true)
    }

    /// Opens a book; without `styles` its CSS is ignored (chapters are
    /// then converted into a cache of their own).
    pub fn open_with(
        storage: &dyn Storage,
        path: &str,
        styles: bool,
    ) -> Result<Self, DocumentError> {
        let archive = ZipArchive::open(storage, path).map_err(package::error)?;
        let cache = alloc::format!("{}/{:016x}", Env::plugin_dir(PLUGIN.as_str()), fnv(path));
        let size = storage.size(path)?;
        let package = match package::load(storage, &cache, size) {
            Some(package) => package,
            None => {
                // Another book, file version or cache format: start over.
                let plain = plain_cache(&cache);
                package::invalidate(storage, &cache, |section| {
                    let _ = storage.remove(&paths(&plain, section).end);
                    paths(&cache, section).end
                });
                package::build(storage, &archive, &cache, size)?
            }
        };
        let chapters = if styles {
            cache.clone()
        } else {
            plain_cache(&cache)
        };
        let mut metadata = package.metadata;
        if metadata.title.is_none() {
            let name = path.rsplit('/').next().unwrap_or(path);
            metadata.title = Some(String::from(name.rsplit_once('.').map_or(name, |(s, _)| s)));
        }
        Ok(EpubDocument {
            lengths: package.spine.iter().map(|_| Cell::new(UNKNOWN)).collect(),
            spine: package.spine,
            metadata,
            toc_len: package.toc_len,
            cover: package.cover,
            archive,
            cache,
            chapters,
            styles,
        })
    }

    fn paths(&self, section: u32) -> CachePaths {
        paths(&self.chapters, section)
    }

    /// The element path of each block of a chapter (converted if needed),
    /// by offset.
    fn positions(&self, storage: &dyn Storage, section: u32) -> Option<Vec<(u32, String)>> {
        let (paths, ..) = self.chapter(storage, section).ok()?;
        let data = quireink_sdk::read_file(storage, &paths.positions, 1 << 20).ok()?;
        let text = String::from_utf8_lossy(&data);
        Some(
            text.lines()
                .filter_map(|line| {
                    let (offset, path) = line.split_once('\t')?;
                    Some((offset.parse().ok()?, String::from(path)))
                })
                .collect(),
        )
    }

    fn length(&self, section: u32) -> Option<u32> {
        let length = self.lengths.get(section as usize)?.get();
        (length != UNKNOWN).then_some(length)
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
                let entry = self
                    .archive
                    .entry_by_record(storage, self.spine[section as usize].record)
                    .map_err(package::error)?;
                let rules = if self.styles {
                    package::styles(storage, &self.cache)
                } else {
                    Default::default()
                };
                convert::convert(storage, &self.archive, &entry, &paths, rules, self.styles)?
            }
        };
        if let Some(slot) = self.lengths.get(section as usize) {
            slot.set(length);
        }
        let blocks = (storage.size(&paths.index)? / u64::from(INDEX)) as u32;
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
        let mut entry = [0u8; INDEX as usize];
        storage.read_at(&paths.index, u64::from(i * INDEX), &mut entry)?;
        let start = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let end = if i + 1 < blocks {
            u32_at(storage, &paths.index, u64::from((i + 1) * INDEX))?
        } else {
            length
        };
        let mut header = [0u8; HEADER as usize];
        let at = start.checked_sub(HEADER).ok_or(DocumentError::Malformed(
            alloc::borrow::Cow::Borrowed("cache index"),
        ))?;
        storage.read_at(&paths.text, u64::from(at), &mut header)?;
        let len = usize::from(u16::from_le_bytes([header[4], header[5]]));
        let run_count = usize::from(u16::from_le_bytes([header[6], header[7]]));
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
        let mut runs = Vec::new();
        if run_count > 0 {
            let mut data = alloc::vec![0u8; run_count * convert::RUN];
            let read = storage.read_at(&paths.text, u64::from(start) + len as u64, &mut data)?;
            runs = data[..read]
                .chunks_exact(convert::RUN)
                .map(|r| Run {
                    start: u32::from(u16::from_le_bytes([r[0], r[1]])),
                    end: u32::from(u16::from_le_bytes([r[2], r[3]])),
                    style: TextStyle::from_bits(r[4]),
                })
                .collect();
        }
        let mut kind = convert::decode_kind(header[0], header[1]);
        let mut text = String::from_utf8_lossy(&text).into_owned();
        let mut image = None;
        if kind == BlockKind::Image {
            // `cache path<TAB>alternative text`
            let (path, alt) = text.split_once('\t').unwrap_or((&text, ""));
            image = GrayFile::open(storage, path).map(|file| ImageRef {
                path: file.path,
                width: file.width,
                height: file.height,
            });
            let alt = String::from(alt);
            if image.is_none() {
                // Not decodable: its alternative text instead.
                kind = BlockKind::Paragraph;
            }
            text = alt;
        }
        Ok(Block {
            kind,
            continues: convert::continues(header[2]),
            start,
            end,
            text,
            map: OffsetMap::IDENTITY,
            runs,
            align: Align::from_bits(header[3]),
            indent: Some(i16::from_le_bytes([entry[4], entry[5]]))
                .filter(|&i| i != convert::NO_INDENT),
            space: Some(u16::from_le_bytes([entry[6], entry[7]]))
                .filter(|&s| s != convert::NO_SPACE),
            size: Some(u16::from_le_bytes([entry[8], entry[9]])).filter(|&s| s != 0),
            image,
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

    fn cover(&self, storage: &dyn Storage) -> Result<Option<ImageRef>, DocumentError> {
        let Some(source) = &self.cover else {
            return Ok(None);
        };
        let path = alloc::format!("{}/cover.gray", self.cache);
        if GrayFile::open(storage, &path).is_none() {
            convert::decode_image(
                storage,
                &self.archive,
                source,
                &path,
                &self.cache,
                convert::COVER_SIDE,
            )?;
        }
        Ok(GrayFile::open(storage, &path).map(|image| ImageRef {
            path: image.path,
            width: image.width,
            height: image.height,
        }))
    }

    fn sections(&self) -> u32 {
        self.spine.len() as u32
    }

    fn section_len(&self, section: u32) -> u32 {
        self.length(section)
            .unwrap_or_else(|| self.spine.get(section as usize).map_or(0, |c| c.size))
    }

    /// Estimated from the chapters' sizes in the archive, so it needs no
    /// conversion of unread chapters.
    fn xpointer(&self, storage: &dyn Storage, at: Locator) -> Option<String> {
        let positions = self.positions(storage, at.section)?;
        let path = positions
            .iter()
            .take_while(|(offset, _)| *offset <= at.offset)
            .last()
            .map_or("", |(_, path)| path.as_str());
        let path = if path.is_empty() {
            String::new()
        } else {
            alloc::format!("/{path}")
        };
        Some(alloc::format!(
            "/body/DocFragment[{}]/body{path}",
            at.section + 1
        ))
    }

    fn locate_xpointer(&self, storage: &dyn Storage, xpointer: &str) -> Option<Locator> {
        let (section, target) = parse_xpointer(xpointer)?;
        if section >= self.sections() {
            return None;
        }
        let positions = self.positions(storage, section)?;
        let inside = |path: &str, of: &str| {
            of.is_empty() || path == of || path.strip_prefix(of).is_some_and(|r| r.starts_with('/'))
        };
        // The element's first block, else the block it is part of.
        let offset = positions
            .iter()
            .find(|(_, path)| inside(path, &target))
            .or_else(|| {
                positions
                    .iter()
                    .rev()
                    .find(|(_, path)| inside(&target, path))
            })
            .map_or(0, |(offset, _)| *offset);
        Some(Locator::new(section, offset))
    }

    fn fonts(&self, storage: &dyn Storage) -> Vec<EmbeddedFont> {
        if !self.styles {
            return Vec::new();
        }
        package::fonts(storage, &self.cache)
            .1
            .into_iter()
            .map(|f| EmbeddedFont {
                family: f.family,
                bold: f.bold,
                italic: f.italic,
            })
            .collect()
    }

    fn font_data(&self, storage: &dyn Storage, index: usize) -> Result<Vec<u8>, DocumentError> {
        let (identifier, fonts) = package::fonts(storage, &self.cache);
        let font = fonts
            .get(index)
            .ok_or(DocumentError::Malformed(Cow::Borrowed("no such font")))?;
        let entry = self
            .archive
            .entry(storage, &font.path)
            .map_err(package::error)?
            .ok_or(DocumentError::Malformed(Cow::Borrowed("font file missing")))?;
        if u64::from(entry.size) > MAX_FONT {
            return Err(DocumentError::OutOfMemory);
        }
        let mut data = Vec::with_capacity(entry.size as usize);
        self.archive
            .stream(storage, &entry, &mut |chunk| {
                data.extend_from_slice(chunk);
                true
            })
            .map_err(package::error)?;
        obfuscation::reveal(&mut data, font.method, &identifier);
        Ok(data)
    }

    fn progress(&self, at: Locator) -> f32 {
        let total: u64 = self.spine.iter().map(|c| u64::from(c.size)).sum();
        if total == 0 {
            return 0.0;
        }
        let section = (at.section as usize).min(self.spine.len());
        let before: u64 = self.spine[..section]
            .iter()
            .map(|c| u64::from(c.size))
            .sum();
        let within = match (self.spine.get(section), self.length(section as u32)) {
            (Some(chapter), Some(length)) if length > 0 => {
                u64::from(chapter.size) * u64::from(at.offset.min(length)) / u64::from(length)
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
            if u32_at(storage, &paths.index, u64::from(mid * INDEX))? <= at.offset {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let i = low.saturating_sub(1);
        self.read_block(storage, &paths, length, blocks, i)
            .map(Some)
    }

    fn toc_len(&self, _: &dyn Storage) -> u32 {
        self.toc_len
    }

    /// Points at the start of the entry's chapter; finding an anchor needs
    /// the chapter converted, which [`Document::toc_target`] does.
    fn toc_entry(&self, storage: &dyn Storage, index: u32) -> Option<TocEntry> {
        let entry = package::toc_entry(storage, &self.cache, index)?;
        Some(TocEntry {
            title: entry.title,
            level: entry.level,
            at: Locator::new(entry.section, 0),
        })
    }

    fn toc_target(&self, storage: &dyn Storage, index: u32) -> Option<Locator> {
        let entry = package::toc_entry(storage, &self.cache, index)?;
        let offset = entry
            .fragment
            .as_deref()
            .and_then(|id| self.anchor(storage, entry.section, id))
            .unwrap_or(0);
        Some(Locator::new(entry.section, offset))
    }
}

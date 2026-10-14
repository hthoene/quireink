use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::document::{
    Block, Document, DocumentError, EmbeddedFont, ImageRef, Link, Locator, Metadata, TocEntry,
};
use quireink_sdk::image::GrayFile;
use quireink_sdk::zip::ZipArchive;
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;
use crate::cached;
use crate::convert::{self, CachePaths, INDEX};
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
        links: alloc::format!("{cache}/{section}.lnk"),
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
        let length = match cached::u32_at(storage, &paths.end, 0) {
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
                let source = convert::Source::Zip {
                    archive: &self.archive,
                    entry: &entry,
                };
                convert::convert(storage, &source, &paths, rules, self.styles)?
            }
        };
        if let Some(slot) = self.lengths.get(section as usize) {
            slot.set(length);
        }
        let blocks = (storage.size(&paths.index)? / u64::from(INDEX)) as u32;
        Ok((paths, length, blocks))
    }

    /// The offset of anchor `id` in a chapter.
    fn anchor(&self, storage: &dyn Storage, section: u32, id: &str) -> Option<u32> {
        let (paths, ..) = self.chapter(storage, section).ok()?;
        cached::anchor(storage, &paths, id)
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
        cached::block_at(storage, &paths, length, blocks, at.offset)
    }

    fn links(&self, storage: &dyn Storage, section: u32, from: u32, to: u32) -> Vec<Link> {
        match self.chapter(storage, section) {
            Ok((paths, ..)) => cached::links(storage, &paths, from, to),
            Err(_) => Vec::new(),
        }
    }

    fn link_target(&self, storage: &dyn Storage, href: &str) -> Option<Locator> {
        if quireink_sdk::document::is_external(href) {
            return None;
        }
        let (path, fragment) = match href.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (href, None),
        };
        let entry = self.archive.entry(storage, path).ok()??;
        let section = self.spine.iter().position(|c| c.record == entry.record())? as u32;
        let offset = fragment
            .and_then(|id| self.anchor(storage, section, id))
            .unwrap_or(0);
        Some(Locator::new(section, offset))
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

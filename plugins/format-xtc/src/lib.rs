//! XTC and XTCH: the pre-rendered page format of the Xteink readers (as
//! made by converters such as `epub2xtc` or `cbz2xtc`).
//!
//! A container (`XTC\0` or `XTCH`) holds a header, optional metadata
//! (title, author, language), optional chapters and an index of pages;
//! each page is a picture of its own:
//!
//! - `XTG\0`: one bit a pixel, row by row (most significant bit left; a
//!   set bit is white);
//! - `XTH\0`: four grays in two bit planes, column by column from the
//!   right, eight rows a byte (most significant bit on top).
//!
//! Each page is a section of one image block, decoded into the gray cache
//! when it is first shown. Pages are decoded in bands of rows, so a page
//! never needs more than a few kilobytes, also on the X4.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use quireink_sdk::document::{
    Align, Block, BlockKind, Document, DocumentError, ImageRef, Locator, Metadata, OffsetMap,
    TocEntry,
};
use quireink_sdk::image::{GrayFile, GrayWriter, ImageError, ImageSink};
use quireink_sdk::{
    Env, FormatSpec, Id, Manifest, Plugin, RegisterError, Registrar, SDK_VERSION, Storage, Version,
};

pub const PLUGIN: Id = Id::new_static("format-xtc");
pub const FORMAT: Id = Id::new_static("format-xtc.xtc");

const XTC: &[u8; 4] = b"XTC\0";
const XTCH: &[u8; 4] = b"XTCH";
const XTG: &[u8; 4] = b"XTG\0";
const XTH: &[u8; 4] = b"XTH\0";
const HEADER: usize = 48;
const METADATA: u64 = 256;
const CHAPTER: u64 = 96;
const INDEX_ENTRY: u64 = 16;
const PAGE_HEADER: u64 = 22;
/// Rows of a gray page decoded at a time.
const BAND: u32 = 32;
/// Bytes read from storage at a time.
const WINDOW: usize = 4096;
/// Chapters kept at most.
const MAX_CHAPTERS: u64 = 1000;

fn malformed(what: &'static str) -> DocumentError {
    DocumentError::Malformed(Cow::Borrowed(what))
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[at..at + 8]);
    u64::from_le_bytes(bytes)
}

/// A zero-terminated UTF-8 string of a fixed-size field.
fn text(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_string()
}

fn read_exact(
    storage: &dyn Storage,
    path: &str,
    at: u64,
    out: &mut [u8],
) -> Result<(), DocumentError> {
    let mut filled = 0;
    while filled < out.len() {
        let n = storage.read_at(path, at + filled as u64, &mut out[filled..])?;
        if n == 0 {
            return Err(malformed("file too short"));
        }
        filled += n;
    }
    Ok(())
}

/// Reads a file through a window, so many small reads near each other
/// cost one read of storage.
struct Window<'a> {
    storage: &'a dyn Storage,
    path: &'a str,
    start: u64,
    len: usize,
    buf: Vec<u8>,
}

impl<'a> Window<'a> {
    fn new(storage: &'a dyn Storage, path: &'a str) -> Self {
        Self {
            storage,
            path,
            start: 0,
            len: 0,
            buf: vec![0; WINDOW],
        }
    }

    /// The byte at `at`, 0 past the end.
    fn byte(&mut self, at: u64) -> u8 {
        if at < self.start || at >= self.start + self.len as u64 {
            self.start = at;
            self.len = self
                .storage
                .read_at(self.path, at, &mut self.buf)
                .unwrap_or(0);
            if self.len == 0 {
                return 0;
            }
        }
        self.buf[(at - self.start) as usize]
    }
}

/// An open XTC or XTCH book.
#[derive(Debug)]
pub struct XtcDocument {
    path: String,
    pages: u32,
    index: u64,
    metadata: Metadata,
    /// Chapters: title and first page.
    chapters: Vec<(String, u32)>,
    cover: u32,
    cache: String,
}

impl XtcDocument {
    pub fn open(storage: &dyn Storage, path: &str) -> Result<Self, DocumentError> {
        let mut head = [0u8; HEADER];
        read_exact(storage, path, 0, &mut head)?;
        if &head[..4] != XTC && &head[..4] != XTCH {
            return Err(malformed("not an XTC book"));
        }
        let pages = u32::from(u16_at(&head, 6));
        let has_metadata = head[9] != 0;
        let has_chapters = head[11] != 0;
        let metadata_at = u64_at(&head, 16);
        let index = u64_at(&head, 24);
        if pages == 0 {
            return Err(malformed("no pages"));
        }
        let mut metadata = Metadata::default();
        let mut cover = 0;
        let mut chapter_count = None;
        if has_metadata && metadata_at > 0 {
            let mut m = [0u8; METADATA as usize];
            read_exact(storage, path, metadata_at, &mut m)?;
            metadata.title = Some(text(&m[..128])).filter(|t| !t.is_empty());
            metadata.authors = Some(text(&m[128..192]))
                .filter(|a| !a.is_empty())
                .into_iter()
                .collect();
            metadata.language = Some(text(&m[224..240])).filter(|l| !l.is_empty());
            let page = u32::from(u16_at(&m, 244));
            if page < pages {
                cover = page;
            }
            chapter_count = Some(u64::from(u16_at(&m, 246)));
        }
        let mut chapters = Vec::new();
        if has_chapters {
            let at = if has_metadata && metadata_at > 0 {
                metadata_at + METADATA
            } else {
                HEADER as u64
            };
            let room = index.saturating_sub(at) / CHAPTER;
            let count = chapter_count.unwrap_or(room).min(room).min(MAX_CHAPTERS);
            let mut c = [0u8; CHAPTER as usize];
            for i in 0..count {
                read_exact(storage, path, at + i * CHAPTER, &mut c)?;
                let start = u32::from(u16_at(&c, 80));
                if start < pages {
                    chapters.push((text(&c[..80]), start));
                }
            }
        }
        if metadata.title.is_none() {
            let name = path.rsplit('/').next().unwrap_or(path);
            metadata.title = Some(name.rsplit_once('.').map_or(name, |(s, _)| s).to_string());
        }
        let cache = format!(
            "{}/{:016x}",
            Env::plugin_dir(PLUGIN.as_str()),
            fnv(path.as_bytes())
        );
        // Pages decoded from another file of the same name go.
        let stamp = format!("{} {pages}\n", storage.size(path)?);
        let stamp_path = format!("{cache}/source");
        if quireink_sdk::read_file(storage, &stamp_path, 64).ok()
            != Some(stamp.clone().into_bytes())
        {
            let _ = storage.remove_dir_all(&cache);
            let _ = storage.create_dir(&cache);
            storage.write(&stamp_path, stamp.as_bytes())?;
        }
        Ok(Self {
            path: path.to_string(),
            pages,
            index,
            metadata,
            chapters,
            cover,
            cache,
        })
    }

    /// Page `page` as a gray picture in the cache, decoded if needed.
    fn page(&self, storage: &dyn Storage, page: u32) -> Result<ImageRef, DocumentError> {
        let cache = format!("{}/p{page}.gray", self.cache);
        if GrayFile::open(storage, &cache).is_none() {
            let mut entry = [0u8; INDEX_ENTRY as usize];
            read_exact(
                storage,
                &self.path,
                self.index + u64::from(page) * INDEX_ENTRY,
                &mut entry,
            )?;
            let offset = u64_at(&entry, 0);
            let mut writer = GrayWriter::new(storage, &cache);
            let result = decode_page(storage, &self.path, offset, &mut writer)
                .and_then(|()| writer.finish().map_err(image_error));
            if result.is_err() {
                let _ = storage.remove(&cache);
            }
            result?;
        }
        let file = GrayFile::open(storage, &cache).ok_or(malformed("page"))?;
        Ok(ImageRef {
            path: file.path,
            width: file.width,
            height: file.height,
        })
    }
}

fn image_error(e: ImageError) -> DocumentError {
    DocumentError::Malformed(Cow::Owned(format!("{e}")))
}

/// Decodes the page picture at `offset` of `path` into `sink`.
fn decode_page(
    storage: &dyn Storage,
    path: &str,
    offset: u64,
    sink: &mut dyn ImageSink,
) -> Result<(), DocumentError> {
    let mut head = [0u8; PAGE_HEADER as usize];
    read_exact(storage, path, offset, &mut head)?;
    let (width, height) = (u32::from(u16_at(&head, 4)), u32::from(u16_at(&head, 6)));
    let size = u64::from(u32_at(&head, 10));
    if width == 0 || height == 0 {
        return Err(malformed("empty page"));
    }
    let data = offset + PAGE_HEADER;
    let mut window = Window::new(storage, path);
    let mut row = vec![0u8; width as usize];
    sink.start(width, height).map_err(image_error)?;
    if &head[..4] == XTG {
        // Rows of bits, a set bit white.
        let stride = u64::from(width.div_ceil(8));
        for y in 0..height {
            for x in 0..width {
                let byte = window.byte(data + u64::from(y) * stride + u64::from(x / 8));
                let on = byte >> (7 - (x % 8)) & 1;
                row[x as usize] = if on == 1 { 255 } else { 0 };
            }
            sink.row(&row).map_err(image_error)?;
        }
        return Ok(());
    }
    if &head[..4] != XTH {
        return Err(malformed("page format"));
    }
    // Two bit planes; in each, columns from the right, eight rows a byte.
    let column = u64::from(height.div_ceil(8));
    let plane = size / 2;
    let mut band = vec![0u8; (BAND * width) as usize];
    let mut y0 = 0;
    while y0 < height {
        let rows = BAND.min(height - y0);
        band[..(rows * width) as usize].fill(0);
        for (p, shift) in [(0u64, 1u8), (1, 0)] {
            for x in 0..width {
                let base = data + p * plane + u64::from(width - 1 - x) * column;
                for y in y0..y0 + rows {
                    let byte = window.byte(base + u64::from(y / 8));
                    let bit = byte >> (7 - (y % 8)) & 1;
                    band[((y - y0) * width + x) as usize] |= bit << shift;
                }
            }
        }
        for r in 0..rows {
            for (out, &raw) in row
                .iter_mut()
                .zip(&band[(r * width) as usize..((r + 1) * width) as usize])
            {
                // 00 white, 01 dark gray, 10 light gray, 11 black.
                *out = [255, 85, 170, 0][usize::from(raw & 3)];
            }
            sink.row(&row).map_err(image_error)?;
        }
        y0 += rows;
    }
    Ok(())
}

impl Document for XtcDocument {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn whole_pages(&self) -> bool {
        true
    }

    fn sections(&self) -> u32 {
        self.pages
    }

    fn section_len(&self, _: u32) -> u32 {
        1
    }

    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError> {
        if at.section >= self.pages || at.offset >= 1 {
            return Ok(None);
        }
        let image = self.page(storage, at.section).ok();
        let (kind, text) = match &image {
            Some(_) => (BlockKind::Image, String::new()),
            None => (BlockKind::Paragraph, format!("{}", at.section + 1)),
        };
        Ok(Some(Block {
            kind,
            continues: false,
            start: 0,
            end: 1,
            text,
            map: OffsetMap::IDENTITY,
            runs: Vec::new(),
            align: Align::Center,
            indent: None,
            space: None,
            size: None,
            image,
        }))
    }

    fn toc_len(&self, _: &dyn Storage) -> u32 {
        self.chapters.len() as u32
    }

    fn toc_entry(&self, _: &dyn Storage, index: u32) -> Option<TocEntry> {
        let (title, page) = self.chapters.get(index as usize)?;
        Some(TocEntry {
            title: title.clone(),
            level: 0,
            at: Locator::new(*page, 0),
        })
    }

    fn cover(&self, storage: &dyn Storage) -> Result<Option<ImageRef>, DocumentError> {
        Ok(self.page(storage, self.cover).ok())
    }
}

/// The XTC format plugin.
#[derive(Debug, Default)]
pub struct XtcFormat;

impl Plugin for XtcFormat {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("XTC"),
            description: Cow::Borrowed(
                "Opens XTC and XTCH books, the Xteink readers' pre-rendered pages.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_format(FormatSpec {
            id: FORMAT,
            title: Cow::Borrowed("format-xtc.title"),
            extensions: vec![Cow::Borrowed("xtc"), Cow::Borrowed("xtch")],
            open: Box::new(|storage, path, _| {
                XtcDocument::open(storage, path).map(|d| Box::new(d) as Box<dyn Document>)
            }),
            prepared: None,
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_i18n::{Catalog, check};
    use quireink_sdk::MemoryStorage;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        assert!(check::compare(&en, &de).is_empty());
    }

    /// A page header and its data.
    fn page(magic: &[u8; 4], width: u16, height: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(magic);
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(data);
        out
    }

    /// An XTC book of `pages` with metadata and one chapter per page.
    fn book(pages: &[Vec<u8>]) -> Vec<u8> {
        let chapters = pages.len() as u64;
        let index = HEADER as u64 + METADATA + CHAPTER * chapters;
        let data = index + INDEX_ENTRY * pages.len() as u64;
        let mut out = Vec::new();
        out.extend_from_slice(XTC);
        out.extend_from_slice(&0x0100u16.to_le_bytes());
        out.extend_from_slice(&(pages.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 1, 0, 1]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(HEADER as u64).to_le_bytes());
        out.extend_from_slice(&index.to_le_bytes());
        out.extend_from_slice(&data.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        let mut meta = vec![0u8; METADATA as usize];
        meta[..5].copy_from_slice(b"Title");
        meta[128..131].copy_from_slice(b"Ann");
        meta[224..226].copy_from_slice(b"de");
        meta[244..246].copy_from_slice(&1u16.to_le_bytes());
        meta[246..248].copy_from_slice(&(chapters as u16).to_le_bytes());
        out.extend_from_slice(&meta);
        for i in 0..chapters {
            let mut c = vec![0u8; CHAPTER as usize];
            let name = format!("Chapter {}", i + 1);
            c[..name.len()].copy_from_slice(name.as_bytes());
            c[80..82].copy_from_slice(&(i as u16).to_le_bytes());
            out.extend_from_slice(&c);
        }
        let mut at = data;
        for p in pages {
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&(p.len() as u32).to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            at += p.len() as u64;
        }
        for p in pages {
            out.extend_from_slice(p);
        }
        out
    }

    fn rows(storage: &dyn Storage, image: &ImageRef) -> Vec<Vec<u8>> {
        let file = GrayFile::open(storage, &image.path).unwrap();
        (0..file.height)
            .map(|y| {
                let mut row = vec![0u8; file.width as usize];
                assert!(file.read_row(storage, y, &mut row));
                row
            })
            .collect()
    }

    #[test]
    fn reads_pages_metadata_and_chapters() {
        // 1 bit: 10 pixels wide, two rows; set bits are white.
        let xtg = page(XTG, 10, 2, &[0b1000_0000, 0b0100_0000, 0xFF, 0xC0]);
        // 2 bits: 2 wide, 9 tall: columns from the right, 2 bytes each.
        // Right column: row 0 black (11), row 1 dark (01); left column:
        // row 0 light (10), row 8 black (11).
        let plane1 = [0b1000_0000, 0, 0b1000_0000, 0b1000_0000];
        let plane2 = [0b1100_0000, 0, 0, 0b1000_0000];
        let mut planes = plane1.to_vec();
        planes.extend_from_slice(&plane2);
        let xth = page(XTH, 2, 9, &planes);
        let storage = MemoryStorage::new().with("b/t.xtc", book(&[xtg, xth]));
        let doc = XtcDocument::open(&storage, "b/t.xtc").unwrap();
        assert_eq!(doc.metadata().title.as_deref(), Some("Title"));
        assert_eq!(doc.metadata().authors, ["Ann"]);
        assert_eq!(doc.sections(), 2);
        assert_eq!(doc.toc_len(&storage), 2);
        assert_eq!(doc.toc_entry(&storage, 1).unwrap().at, Locator::new(1, 0));

        let first = doc.block_at(&storage, Locator::new(0, 0)).unwrap().unwrap();
        let image = first.image.unwrap();
        assert_eq!((image.width, image.height), (10, 2));
        let r = rows(&storage, &image);
        assert_eq!(r[0], [255, 0, 0, 0, 0, 0, 0, 0, 0, 255]);
        assert_eq!(r[1], [255; 10]);

        let second = doc.block_at(&storage, Locator::new(1, 0)).unwrap().unwrap();
        let r = rows(&storage, &second.image.unwrap());
        assert_eq!(r[0], [170, 0]);
        assert_eq!(r[1], [255, 85]);
        assert_eq!(r[8], [0, 255]);
        // The cover is the page the metadata names.
        let cover = doc.cover(&storage).unwrap().unwrap();
        assert_eq!((cover.width, cover.height), (2, 9));
        assert!(
            doc.block_at(&storage, Locator::new(2, 0))
                .unwrap()
                .is_none()
        );
    }
}

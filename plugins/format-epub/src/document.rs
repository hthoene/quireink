use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::document::{
    Block, Document, DocumentError, EmbeddedFont, ImageRef, Link, Locator, Metadata, TocEntry,
};
use quireink_sdk::image::Fit;
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
    /// Pictures in colour (ADR 0015).
    color: bool,
    /// Pictures decoded at most this large.
    image_fit: Fit,
    /// Archive path of the cover image.
    cover: Option<String>,
    /// One's own stylesheets for this book (see [`user_css`]), applied
    /// after the book's.
    user_css: String,
    /// The printed pages of one section, resolved: offset and label.
    pages: core::cell::RefCell<Option<SectionPages>>,
    /// The chapter read last, known: its paths, text length and number of
    /// blocks, and the block found last in it. A page is laid out a block
    /// at a time, the next one mostly right after the last.
    known: core::cell::RefCell<Option<Known>>,
}

/// A converted chapter, as [`EpubDocument::known`] keeps it.
#[derive(Debug)]
struct Known {
    section: u32,
    paths: CachePaths,
    length: u32,
    blocks: u32,
    last: Option<u32>,
}

/// A section and its printed pages: offset and label.
type SectionPages = (u32, Vec<(u32, String)>);

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
        xpointers: alloc::format!("{cache}/{section}.xpt"),
        end: alloc::format!("{cache}/{section}.end"),
    }
}

/// What an XPointer points at: the section, the element path below
/// `<body>`, and the text node in it with the character, if it points
/// into text.
struct XPointer {
    section: u32,
    element: String,
    text: Option<(u32, u32)>,
}

impl XPointer {
    /// The text node as `<n>.xpt` names it, e.g. `div[1]/p[5]/text()[1]`.
    fn node(&self) -> Option<(String, u32)> {
        let (index, char) = self.text?;
        let node = if self.element.is_empty() {
            alloc::format!("text()[{index}]")
        } else {
            alloc::format!("{}/text()[{index}]", self.element)
        };
        Some((node, char))
    }
}

/// The parts of an XPointer such as
/// `/body/DocFragment[3]/body/div/p[5]/text().12`: indexes are filled in
/// (`div` is `div[1]`, `text()` is `text()[1]`).
fn parse_xpointer(xpointer: &str) -> Option<XPointer> {
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
    let mut text = None;
    for segment in rest.split('/').filter(|s| !s.is_empty()) {
        if let Some(node) = segment.strip_prefix("text()") {
            // `text()[2].5`, `text().5`
            let (index, char) = match node.strip_prefix('[') {
                Some(r) => {
                    let (n, r) = r.split_once(']')?;
                    (n.parse::<u32>().ok()?, r)
                }
                None => (1, node),
            };
            let char = char.strip_prefix('.').and_then(|c| c.parse::<u32>().ok());
            text = char.map(|char| (index, char));
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
    Some(XPointer {
        section: index.checked_sub(1)?,
        element: segments.join("/"),
        text,
    })
}

/// The lines of the file at `path` in order, read a piece at a time,
/// until `each` returns `false`.
fn each_line(storage: &dyn Storage, path: &str, mut each: impl FnMut(&str) -> bool) {
    let mut chunk = [0u8; 512];
    let mut line: Vec<u8> = Vec::new();
    let mut at = 0u64;
    loop {
        let Ok(n) = storage.read_at(path, at, &mut chunk) else {
            return;
        };
        if n == 0 {
            break;
        }
        at += n as u64;
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                if !each(&String::from_utf8_lossy(&line)) {
                    return;
                }
                line.clear();
            } else {
                line.push(byte);
            }
        }
    }
    if !line.is_empty() {
        each(&String::from_utf8_lossy(&line));
    }
}

/// A line of `<n>.xpt`: offset, text node, character.
fn origin(line: &str) -> Option<(u32, &str, u32)> {
    let mut fields = line.split('\t');
    let offset = fields.next()?.parse().ok()?;
    let node = fields.next()?;
    let char = fields.next()?.parse().ok()?;
    Some((offset, node, char))
}

/// Where chapters converted without the book's styles are kept.
/// The cache of a book.
fn book_cache(path: &str) -> String {
    alloc::format!("{}/{:016x}", Env::plugin_dir(PLUGIN.as_str()), fnv(path))
}

/// The cache of a book's chapters converted with its styles or not, and
/// one's own stylesheets: chapters converted with them go into a cache of
/// their own, so a changed stylesheet applies at once.
fn chapters_cache(
    storage: &dyn Storage,
    path: &str,
    cache: &str,
    styles: bool,
    plugin_css: &str,
) -> (String, String) {
    let user_css = if styles {
        user_css(storage, path, plugin_css)
    } else {
        String::new()
    };
    let chapters = if !styles {
        plain_cache(cache)
    } else if user_css.is_empty() {
        String::from(cache)
    } else {
        alloc::format!("{cache}/css-{:016x}", fnv(&user_css))
    };
    (chapters, user_css)
}

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
        Self::open_styled(storage, path, styles, "", false, convert::IMAGE_FIT)
    }

    /// Opens a book as `options` say: with its styles or not, and with
    /// stylesheets to apply after its own (e.g. the plugins').
    pub fn open_options(
        storage: &dyn Storage,
        path: &str,
        options: &quireink_sdk::OpenOptions,
    ) -> Result<Self, DocumentError> {
        Self::open_styled(
            storage,
            path,
            options.publisher_styles,
            &options.css,
            options.color,
            options.image_fit,
        )
    }

    /// Whether the book opens at once as `options` say: its first chapter
    /// is converted. Converting it (and decoding its pictures, often a
    /// large cover) takes a while the first time.
    pub fn prepared(
        storage: &dyn Storage,
        path: &str,
        options: &quireink_sdk::OpenOptions,
    ) -> bool {
        let cache = book_cache(path);
        let (chapters, _) = chapters_cache(
            storage,
            path,
            &cache,
            options.publisher_styles,
            &options.css,
        );
        storage.exists(&paths(&chapters, 0).end)
    }

    fn open_styled(
        storage: &dyn Storage,
        path: &str,
        styles: bool,
        plugin_css: &str,
        color: bool,
        image_fit: Fit,
    ) -> Result<Self, DocumentError> {
        let archive = ZipArchive::open(storage, path).map_err(package::error)?;
        let cache = book_cache(path);
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
        let (chapters, user_css) = chapters_cache(storage, path, &cache, styles, plugin_css);
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
            color,
            image_fit,
            user_css,
            pages: core::cell::RefCell::new(None),
            known: core::cell::RefCell::new(None),
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
                let mut rules = if self.styles {
                    package::styles(storage, &self.cache)
                } else {
                    Default::default()
                };
                // One's own after the book's: they win.
                if !self.user_css.is_empty() {
                    rules.add_own(&self.user_css);
                }
                let source = convert::Source::Zip {
                    archive: &self.archive,
                    entry: &entry,
                };
                convert::convert(
                    storage,
                    &source,
                    &paths,
                    rules,
                    self.styles,
                    self.color,
                    self.image_fit,
                )?
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

/// Longest stylesheet of one's own that is read.
const MAX_USER_CSS: usize = 64 * 1024;

/// The stylesheets for the book at `path` after its own: `plugins'`
/// (e.g. a theme's), then one's own: `styles.css` in its folder (for all
/// books there), then the one named like the book (`Alice.css` for
/// `Alice.epub`). The later win.
pub(crate) fn user_css(storage: &dyn Storage, path: &str, plugins: &str) -> String {
    let folder = quireink_sdk::files::parent(path);
    let name = quireink_sdk::files::name(path);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let mut css = String::from(plugins);
    if !css.is_empty() {
        css.push('\n');
    }
    for file in [String::from("styles.css"), alloc::format!("{stem}.css")] {
        let file = quireink_sdk::files::join(folder, &file);
        if let Ok(data) = quireink_sdk::read_file(storage, &file, MAX_USER_CSS) {
            css.push_str(&String::from_utf8_lossy(&data));
            css.push('\n');
        }
    }
    css
}

impl EpubDocument {
    /// The printed pages listed for `section`, with their offsets; the
    /// label of the last page before it.
    fn section_pages(
        &self,
        storage: &dyn Storage,
        section: u32,
    ) -> (Option<String>, Vec<(u32, String)>) {
        let Ok(data) = quireink_sdk::read_file(
            storage,
            &alloc::format!("{}/pages.txt", self.cache),
            1 << 20,
        ) else {
            return (None, Vec::new());
        };
        let text = String::from_utf8_lossy(&data);
        let mut before = None;
        let mut here = Vec::new();
        for line in text.lines() {
            let mut f = line.splitn(3, '\t');
            let (Some(s), Some(fragment), Some(label)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let Ok(s) = s.parse::<u32>() else {
                continue;
            };
            if s < section {
                before = Some(String::from(label));
            } else if s == section
                && let Some(offset) = self.anchor(storage, section, fragment)
            {
                here.push((offset, String::from(label)));
            }
        }
        here.sort_by_key(|(offset, _)| *offset);
        (before, here)
    }
}

impl Document for EpubDocument {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn page_label(&self, storage: &dyn Storage, at: Locator) -> Option<String> {
        // One section's pages are resolved once.
        let mut pages = self.pages.borrow_mut();
        if pages.as_ref().is_none_or(|(s, _)| *s != at.section) {
            let (before, mut here) = self.section_pages(storage, at.section);
            if let Some(before) = before {
                here.insert(0, (0, before));
            }
            *pages = Some((at.section, here));
        }
        let (_, here) = pages.as_ref()?;
        here.iter()
            .take_while(|(offset, _)| *offset <= at.offset)
            .last()
            .map(|(_, label)| label.clone())
    }

    fn cover(&self, storage: &dyn Storage) -> Result<Option<ImageRef>, DocumentError> {
        let Some(source) = &self.cover else {
            return Ok(None);
        };
        let base = alloc::format!("{}/cover", self.cache);
        if !crate::pictures::has(storage, &base, self.color) {
            convert::decode_image(
                storage,
                &self.archive,
                source,
                &crate::pictures::file(&base, self.color),
                &self.cache,
                convert::COVER_FIT,
                self.color,
            )?;
        }
        Ok(crate::pictures::open(storage, &base, self.color))
    }

    fn sections(&self) -> u32 {
        self.spine.len() as u32
    }

    fn section_len(&self, section: u32) -> u32 {
        self.length(section)
            .unwrap_or_else(|| self.spine.get(section as usize).map_or(0, |c| c.size))
    }

    /// Down to the character in its text node, as KOReader counts them,
    /// where it is in text; else the element of its block.
    fn xpointer(&self, storage: &dyn Storage, at: Locator) -> Option<String> {
        let positions = self.positions(storage, at.section)?;
        let (block, path) = positions
            .iter()
            .take_while(|(offset, _)| *offset <= at.offset)
            .last()
            .map_or((0, ""), |(offset, path)| (*offset, path.as_str()));
        let fragment = alloc::format!("/body/DocFragment[{}]/body", at.section + 1);
        // The text node the block's text at `at` comes from.
        let paths = self.paths(at.section);
        let mut found: Option<(u32, String, u32)> = None;
        each_line(storage, &paths.xpointers, |line| {
            let Some((offset, node, char)) = origin(line) else {
                return true;
            };
            if offset > at.offset {
                return false;
            }
            found = Some((offset, String::from(node), char));
            true
        });
        if let Some((offset, node, char)) = found.filter(|(offset, ..)| *offset >= block) {
            let mut text = alloc::vec![0u8; (at.offset - offset) as usize];
            let n = storage
                .read_at(&paths.text, u64::from(offset), &mut text)
                .ok()?;
            let chars = String::from_utf8_lossy(&text[..n]).chars().count() as u32;
            return Some(alloc::format!("{fragment}/{node}.{}", char + chars));
        }
        if path.is_empty() {
            Some(fragment)
        } else {
            Some(alloc::format!("{fragment}/{path}"))
        }
    }

    fn locate_xpointer(&self, storage: &dyn Storage, xpointer: &str) -> Option<Locator> {
        let pointer = parse_xpointer(xpointer)?;
        let section = pointer.section;
        if section >= self.sections() {
            return None;
        }
        let positions = self.positions(storage, section)?;
        // Exactly, where it points into text QuireInk shows.
        if let Some((node, char)) = pointer.node() {
            let paths = self.paths(section);
            let mut found: Option<(u32, u32)> = None;
            each_line(storage, &paths.xpointers, |line| {
                if let Some((offset, at, from)) = origin(line)
                    && at == node
                    && from <= char
                {
                    found = Some((offset, char - from));
                }
                true
            });
            if let Some((offset, chars)) = found {
                let mut text = alloc::vec![0u8; (chars as usize * 4).min(16 * 1024)];
                let n = storage
                    .read_at(&paths.text, u64::from(offset), &mut text)
                    .ok()?;
                let text = String::from_utf8_lossy(&text[..n]);
                let bytes: usize = text.chars().take(chars as usize).map(char::len_utf8).sum();
                return Some(Locator::new(section, offset + bytes as u32));
            }
        }
        let target = pointer.element;
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
        let mut data = Vec::new();
        data.try_reserve_exact(entry.size as usize)
            .map_err(|_| DocumentError::OutOfMemory)?;
        self.archive
            .stream(storage, &entry, &mut |chunk| {
                data.extend_from_slice(chunk);
                true
            })
            .map_err(package::error)?;
        obfuscation::reveal(&mut data, font.method, &identifier);
        if data.starts_with(crate::woff::SIGNATURE) {
            return crate::woff::to_font(&data, MAX_FONT as usize)
                .ok_or(DocumentError::Malformed(Cow::Borrowed("WOFF font")));
        }
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
        let mut known = self.known.borrow_mut();
        if known.as_ref().is_none_or(|k| k.section != at.section) {
            let (paths, length, blocks) = self.chapter(storage, at.section)?;
            *known = Some(Known {
                section: at.section,
                paths,
                length,
                blocks,
                last: None,
            });
        }
        let Some(k) = known.as_mut() else {
            return Ok(None);
        };
        cached::block_at(
            storage,
            &k.paths,
            k.length,
            k.blocks,
            at.offset,
            self.color,
            &mut k.last,
        )
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

//! FictionBook 2 (`.fb2`): one XML file with the book's description, its
//! bodies of nested sections and its pictures as base64 `<binary>`
//! elements.
//!
//! - Metadata from `<title-info>`: title, authors, language, series
//!   (`<sequence>`), keywords as subjects, the annotation as description
//!   and the cover (`<coverpage>`).
//! - The text is converted like an EPUB chapter (see [`crate::convert`]),
//!   FictionBook's elements read as the HTML ones they look like
//!   (`<emphasis>` as `<i>`, `<epigraph>` as `<blockquote>`, …); section
//!   titles become headings by their depth and the table of contents.
//!   Notes (`<body name="notes">`) follow the text, so note links work as
//!   in EPUBs.
//! - Pictures are decoded from their `<binary>` after the text.
//! - UTF-8, Windows-1251 (common for Russian books) and Windows-1252 /
//!   ISO-8859-1, as the XML declaration says.
//!
//! What is read once (metadata, contents) is cached beside the converted
//! text, so opening a book again does not read the whole file.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::document::{
    Block, Document, DocumentError, ImageRef, Link, Locator, Metadata, TocEntry,
};
use quireink_sdk::image::GrayFile;
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;
use crate::cached;
use crate::convert::{self, CachePaths, INDEX, Source};

/// Changes whenever books are read differently, so they are read again.
const FB2_VERSION: u32 = 1;
/// Marks a book whose converted length is not known yet.
const UNKNOWN: u32 = u32::MAX;
/// Contents entries kept at most.
const MAX_TOC: usize = 2000;
/// Longest title of a contents entry kept.
const MAX_TITLE: usize = 200;
/// Anchor ids of section titles: this, then their number.
pub(crate) const TOC_ANCHOR: &str = "fb2-toc-";

/// How the file's bytes are text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
    /// A single-byte encoding: the characters of bytes 128 to 255.
    Single(&'static [char; 128]),
}

/// The encoding the XML declaration names (UTF-8 if none).
fn detect(head: &[u8]) -> Encoding {
    let head = String::from_utf8_lossy(&head[..head.len().min(200)]).to_ascii_lowercase();
    let name = head
        .split("encoding=")
        .nth(1)
        .and_then(|rest| {
            rest.trim_start_matches(['"', '\''])
                .split(['"', '\''])
                .next()
        })
        .unwrap_or("utf-8");
    match name {
        "windows-1251" | "cp1251" => Encoding::Single(&CP1251),
        "windows-1252" | "cp1252" | "iso-8859-1" | "latin1" => Encoding::Single(&CP1252),
        _ => Encoding::Utf8,
    }
}

/// Feeds the file at `path` to `tokenizer` as UTF-8.
pub(crate) fn feed(
    storage: &dyn Storage,
    path: &str,
    encoding: Encoding,
    tokenizer: &mut Tokenizer,
    emit: &mut dyn FnMut(Token<'_>),
) -> Result<(), DocumentError> {
    let mut chunk = [0u8; 1024];
    let mut utf8 = Vec::with_capacity(3 * chunk.len());
    let mut at = 0u64;
    loop {
        let n = storage.read_at(path, at, &mut chunk)?;
        if n == 0 {
            break;
        }
        at += n as u64;
        match encoding {
            Encoding::Utf8 => tokenizer.feed(&chunk[..n], emit),
            Encoding::Single(table) => {
                utf8.clear();
                let mut buf = [0u8; 4];
                for &b in &chunk[..n] {
                    if b < 128 {
                        utf8.push(b);
                    } else {
                        let c = table[usize::from(b - 128)];
                        utf8.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                }
                tokenizer.feed(&utf8, emit);
            }
        }
    }
    tokenizer.finish(emit);
    Ok(())
}

/// The value of an attribute by its name without prefix (`l:href`,
/// `xlink:href`).
pub(crate) fn attr(attrs: &quireink_sdk::xml::Attrs<'_>, name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(n, _)| local_name(n) == name)
        .map(|(_, v)| decode(v).into_owned())
}

/// What one look at the book finds.
#[derive(Debug, Default)]
struct Scan {
    metadata: Metadata,
    /// Section titles: depth (0 for top-level sections) and text.
    toc: Vec<(u8, String)>,
    cover: Option<String>,
}

fn scan(storage: &dyn Storage, path: &str, encoding: Encoding) -> Result<Scan, DocumentError> {
    let mut found = Scan::default();
    let mut stack: Vec<String> = Vec::new();
    let mut text = String::new();
    // An author's name parts, a title being read and where sections are.
    let mut name_parts: Vec<String> = Vec::new();
    let mut nickname = String::new();
    let mut title: Option<String> = None;
    let mut depth = 0u32;
    let mut notes = false;
    let mut annotation: Option<String> = None;
    let mut handle = |token: Token<'_>| match token {
        Token::Start {
            name,
            attrs,
            self_closing,
        } => {
            let tag = local_name(name).to_string();
            let in_info = stack.iter().any(|t| t == "title-info");
            match tag.as_str() {
                "body" => notes = attrs.get("name").as_deref() == Some("notes"),
                "section" => depth += 1,
                "title" if !notes && depth > 0 => title = Some(String::new()),
                "sequence" if in_info && found.metadata.series.is_none() => {
                    found.metadata.series = attr(&attrs, "name").filter(|n| !n.is_empty());
                    found.metadata.series_index =
                        attr(&attrs, "number").filter(|n| !n.is_empty() && n != "0");
                }
                "image" if in_info && stack.last().is_some_and(|t| t == "coverpage") => {
                    found.cover = attr(&attrs, "href").map(|h| h.trim_start_matches('#').into());
                }
                "annotation" if in_info => annotation = Some(String::new()),
                "author" if in_info => {
                    name_parts.clear();
                    nickname.clear();
                }
                _ => {}
            }
            if !self_closing {
                stack.push(tag);
            }
            text.clear();
        }
        Token::End { name } => {
            let tag = local_name(name);
            let in_info = stack.iter().any(|t| t == "title-info");
            let value = clean(&text);
            match tag {
                "book-title" if in_info => found.metadata.title = Some(value),
                "lang" if in_info && !value.is_empty() => found.metadata.language = Some(value),
                "keywords" if in_info => {
                    found.metadata.subjects = value
                        .split([',', ';'])
                        .map(str::trim)
                        .filter(|k| !k.is_empty())
                        .map(String::from)
                        .collect();
                }
                "first-name" | "middle-name" | "last-name" if in_info && !value.is_empty() => {
                    name_parts.push(value);
                }
                "nickname" if in_info => nickname = value,
                "author" if in_info => {
                    let full = if name_parts.is_empty() {
                        core::mem::take(&mut nickname)
                    } else {
                        name_parts.join(" ")
                    };
                    if !full.is_empty() {
                        found.metadata.authors.push(full);
                    }
                }
                "annotation" if in_info => {
                    found.metadata.description = annotation.take().map(|a| clean(&a));
                }
                "section" => depth = depth.saturating_sub(1),
                "title" => {
                    if let Some(t) = title.take()
                        && found.toc.len() < MAX_TOC
                    {
                        let mut t = clean(&t);
                        if t.len() > MAX_TITLE {
                            let mut end = MAX_TITLE;
                            while !t.is_char_boundary(end) {
                                end -= 1;
                            }
                            t.truncate(end);
                        }
                        found.toc.push(((depth.saturating_sub(1)).min(5) as u8, t));
                    }
                }
                // Lines of a title or an annotation are parted.
                "p" => {
                    for open in [title.as_mut(), annotation.as_mut()].into_iter().flatten() {
                        open.push(' ');
                    }
                }
                _ => {}
            }
            if let Some(at) = stack.iter().rposition(|t| t == tag) {
                stack.truncate(at);
            }
            text.clear();
        }
        Token::Text(t) => {
            // Pictures are long and of no use here.
            if stack.last().is_some_and(|t| t == "binary") {
                return;
            }
            let t = decode(&t);
            if text.len() < 4096 {
                text.push_str(&t);
            }
            if let Some(title) = title.as_mut()
                && title.len() < MAX_TITLE * 2
            {
                title.push_str(&t);
            }
            if let Some(a) = annotation.as_mut()
                && a.len() < 4096
            {
                a.push_str(&t);
            }
        }
        Token::CData(_) => {}
    };
    let mut tokenizer = Tokenizer::new();
    feed(storage, path, encoding, &mut tokenizer, &mut handle)?;
    Ok(found)
}

/// Whitespace collapsed.
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decodes base64 piece by piece; other characters are skipped.
#[derive(Debug, Default)]
struct Base64 {
    acc: u32,
    bits: u32,
}

impl Base64 {
    fn feed(&mut self, text: &str, out: &mut Vec<u8>) {
        for b in text.bytes() {
            let v = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' | b'-' => 62,
                b'/' | b'_' => 63,
                _ => continue,
            };
            self.acc = (self.acc << 6) | u32::from(v);
            self.bits += 6;
            if self.bits >= 8 {
                self.bits -= 8;
                out.push((self.acc >> self.bits) as u8);
                self.acc &= (1 << self.bits) - 1;
            }
        }
    }
}

/// Decodes the pictures `wanted` (binary id, cache file, longest side)
/// from their `<binary>` elements: each into a temporary file, then into
/// the cache. One that cannot be decoded is left out.
pub(crate) fn decode_binaries(
    storage: &dyn Storage,
    path: &str,
    encoding: Encoding,
    wanted: &[(String, String, u32)],
    dir: &str,
) -> Result<(), DocumentError> {
    let wanted: Vec<&(String, String, u32)> = wanted
        .iter()
        .filter(|(_, cache, _)| GrayFile::open(storage, cache).is_none())
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let temp = format!("{dir}/binary.tmp");
    let mut current: Option<usize> = None;
    let mut base64 = Base64::default();
    let mut bytes = Vec::new();
    let mut done: Vec<usize> = Vec::new();
    let mut failed = None;
    let mut handle = |token: Token<'_>| match token {
        Token::Start { name, attrs, .. } if local_name(name) == "binary" => {
            let id = attrs.get("id").unwrap_or_default();
            current = wanted.iter().position(|(w, ..)| *w == id);
            if current.is_some() {
                base64 = Base64::default();
                failed = storage.write(&temp, &[]).err().or(failed.take());
            }
        }
        Token::Text(text) if current.is_some() => {
            bytes.clear();
            base64.feed(&text, &mut bytes);
            if failed.is_none() {
                failed = storage.append(&temp, &bytes).err();
            }
        }
        Token::End { name } if local_name(name) == "binary" => {
            if let Some(at) = current.take() {
                let (_, cache, side) = wanted[at];
                if failed.is_none() && convert::decode_file(storage, &temp, cache, *side).is_ok() {
                    done.push(at);
                }
            }
        }
        _ => {}
    };
    let mut tokenizer = Tokenizer::new();
    feed(storage, path, encoding, &mut tokenizer, &mut handle)?;
    let _ = storage.remove(&temp);
    match failed {
        Some(e) => Err(DocumentError::Storage(e)),
        None => Ok(()),
    }
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// An open FictionBook.
#[derive(Debug)]
pub struct Fb2Document {
    path: String,
    paths: CachePaths,
    encoding: Encoding,
    metadata: Metadata,
    toc: Vec<(u8, String)>,
    cover: Option<String>,
    styles: bool,
    /// Size of the file: the length until it is converted.
    size: u32,
    length: Cell<u32>,
}

impl Fb2Document {
    /// Opens a FictionBook; without `styles` it is converted into a cache
    /// of its own (FictionBook has no style sheets, but the setting keeps
    /// caches apart).
    pub fn open(storage: &dyn Storage, path: &str, styles: bool) -> Result<Self, DocumentError> {
        let size = storage.size(path)?;
        let mut head = [0u8; 4096];
        let n = storage.read_at(path, 0, &mut head)?;
        let encoding = detect(&head[..n]);
        let mut cache = format!(
            "{}/fb2-{:016x}",
            Env::plugin_dir(PLUGIN.as_str()),
            fnv(path.as_bytes())
        );
        if !styles {
            cache.push_str("/plain");
        }
        let paths = CachePaths {
            dir: cache.clone(),
            text: format!("{cache}/0.txt"),
            index: format!("{cache}/0.idx"),
            anchors: format!("{cache}/0.anc"),
            links: format!("{cache}/0.lnk"),
            positions: format!("{cache}/0.pos"),
            end: format!("{cache}/0.end"),
        };
        // Read again when the file or the way it is read changed.
        let stamp = format!(
            "{}.{FB2_VERSION} {size} {:016x}\n",
            crate::package::CACHE_VERSION,
            fnv(&head[..n])
        );
        let stamp_path = format!("{cache}/source");
        let info_path = format!("{cache}/book.txt");
        let fresh = quireink_sdk::read_file(storage, &stamp_path, 256).ok()
            == Some(stamp.clone().into_bytes());
        let found = match fresh.then(|| load_info(storage, &info_path)).flatten() {
            Some(found) => found,
            None => {
                let _ = storage.remove(&paths.end);
                let _ = storage.create_dir(&cache);
                let found = scan(storage, path, encoding)?;
                storage.write(&info_path, save_info(&found).as_bytes())?;
                storage.write(&stamp_path, stamp.as_bytes())?;
                found
            }
        };
        let mut metadata = found.metadata;
        if metadata.title.is_none() {
            let name = path.rsplit('/').next().unwrap_or(path);
            metadata.title = Some(name.rsplit_once('.').map_or(name, |(s, _)| s).to_string());
        }
        Ok(Self {
            path: path.to_string(),
            paths,
            encoding,
            metadata,
            toc: found.toc,
            cover: found.cover,
            styles,
            size: u32::try_from(size).unwrap_or(u32::MAX - 1),
            length: Cell::new(UNKNOWN),
        })
    }

    /// Converts the book if needed; returns its text length and number of
    /// blocks.
    fn converted(&self, storage: &dyn Storage) -> Result<(u32, u32), DocumentError> {
        let length = match cached::u32_at(storage, &self.paths.end, 0) {
            Ok(length) => length,
            Err(_) => {
                let source = Source::Fb2 {
                    path: &self.path,
                    encoding: self.encoding,
                };
                convert::convert(
                    storage,
                    &source,
                    &self.paths,
                    Default::default(),
                    self.styles,
                )?
            }
        };
        self.length.set(length);
        let blocks = (storage.size(&self.paths.index)? / u64::from(INDEX)) as u32;
        Ok((length, blocks))
    }
}

/// The cached metadata and contents: `key<TAB>value` lines, contents as
/// `toc<TAB>depth<TAB>title`.
fn save_info(found: &Scan) -> String {
    let one = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let m = &found.metadata;
    let mut out = String::new();
    let mut line = |key: &str, value: &str| {
        out.push_str(&format!("{key}\t{}\n", one(value)));
    };
    for (key, value) in [
        ("title", &m.title),
        ("language", &m.language),
        ("series", &m.series),
        ("series-index", &m.series_index),
        ("description", &m.description),
        ("cover", &found.cover),
    ] {
        if let Some(value) = value {
            line(key, value);
        }
    }
    for author in &m.authors {
        line("author", author);
    }
    for subject in &m.subjects {
        line("subject", subject);
    }
    for (depth, title) in &found.toc {
        out.push_str(&format!("toc\t{depth}\t{}\n", one(title)));
    }
    out
}

fn load_info(storage: &dyn Storage, path: &str) -> Option<Scan> {
    let data = quireink_sdk::read_file(storage, path, 1 << 20).ok()?;
    let text = core::str::from_utf8(&data).ok()?;
    let mut found = Scan::default();
    for line in text.lines() {
        let (key, value) = line.split_once('\t')?;
        let value = value.to_string();
        let m = &mut found.metadata;
        match key {
            "title" => m.title = Some(value),
            "language" => m.language = Some(value),
            "series" => m.series = Some(value),
            "series-index" => m.series_index = Some(value),
            "description" => m.description = Some(value),
            "cover" => found.cover = Some(value),
            "author" => m.authors.push(value),
            "subject" => m.subjects.push(value),
            "toc" => {
                let (depth, title) = value.split_once('\t')?;
                found.toc.push((depth.parse().ok()?, title.to_string()));
            }
            _ => {}
        }
    }
    Some(found)
}

impl Document for Fb2Document {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn sections(&self) -> u32 {
        1
    }

    fn section_len(&self, _: u32) -> u32 {
        match self.length.get() {
            UNKNOWN => self.size,
            length => length,
        }
    }

    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError> {
        if at.section != 0 {
            return Ok(None);
        }
        let (length, blocks) = self.converted(storage)?;
        cached::block_at(storage, &self.paths, length, blocks, at.offset)
    }

    fn toc_len(&self, _: &dyn Storage) -> u32 {
        self.toc.len() as u32
    }

    fn toc_entry(&self, storage: &dyn Storage, index: u32) -> Option<TocEntry> {
        let (level, title) = self.toc.get(index as usize)?;
        Some(TocEntry {
            title: title.clone(),
            level: *level,
            at: self.toc_target(storage, index).unwrap_or(Locator::START),
        })
    }

    fn toc_target(&self, storage: &dyn Storage, index: u32) -> Option<Locator> {
        self.converted(storage).ok()?;
        let id = format!("{TOC_ANCHOR}{index}");
        cached::anchor(storage, &self.paths, &id).map(|offset| Locator::new(0, offset))
    }

    fn cover(&self, storage: &dyn Storage) -> Result<Option<ImageRef>, DocumentError> {
        let Some(id) = &self.cover else {
            return Ok(None);
        };
        let cache = format!("{}/cover.gray", self.paths.dir);
        decode_binaries(
            storage,
            &self.path,
            self.encoding,
            &[(id.clone(), cache.clone(), convert::COVER_SIDE)],
            &self.paths.dir,
        )?;
        Ok(GrayFile::open(storage, &cache).map(|image| ImageRef {
            path: image.path,
            width: image.width,
            height: image.height,
        }))
    }

    fn links(&self, storage: &dyn Storage, section: u32, from: u32, to: u32) -> Vec<Link> {
        if section != 0 || self.converted(storage).is_err() {
            return Vec::new();
        }
        cached::links(storage, &self.paths, from, to)
    }

    fn link_target(&self, storage: &dyn Storage, href: &str) -> Option<Locator> {
        let id = href.rsplit_once('#').map_or(href, |(_, id)| id);
        self.converted(storage).ok()?;
        cached::anchor(storage, &self.paths, id).map(|offset| Locator::new(0, offset))
    }
}

/// Windows-1251 (Cyrillic), bytes 128 to 255.
static CP1251: [char; 128] = [
    'Ђ', 'Ѓ', '‚', 'ѓ', '„', '…', '†', '‡', '€', '‰', 'Љ', '‹', 'Њ', 'Ќ', 'Ћ', 'Џ', //
    'ђ', '‘', '’', '“', '”', '•', '–', '—', '\u{fffd}', '™', 'љ', '›', 'њ', 'ќ', 'ћ', 'џ', //
    '\u{a0}', 'Ў', 'ў', 'Ј', '¤', 'Ґ', '¦', '§', 'Ё', '©', 'Є', '«', '¬', '\u{ad}', '®',
    'Ї', //
    '°', '±', 'І', 'і', 'ґ', 'µ', '¶', '·', 'ё', '№', 'є', '»', 'ј', 'Ѕ', 'ѕ', 'ї', //
    'А', 'Б', 'В', 'Г', 'Д', 'Е', 'Ж', 'З', 'И', 'Й', 'К', 'Л', 'М', 'Н', 'О', 'П', //
    'Р', 'С', 'Т', 'У', 'Ф', 'Х', 'Ц', 'Ч', 'Ш', 'Щ', 'Ъ', 'Ы', 'Ь', 'Э', 'Ю', 'Я', //
    'а', 'б', 'в', 'г', 'д', 'е', 'ж', 'з', 'и', 'й', 'к', 'л', 'м', 'н', 'о', 'п', //
    'р', 'с', 'т', 'у', 'ф', 'х', 'ц', 'ч', 'ш', 'щ', 'ъ', 'ы', 'ь', 'э', 'ю', 'я', //
];

/// Windows-1252 (Western), bytes 128 to 255 (ISO-8859-1 where it has
/// characters).
static CP1252: [char; 128] = [
    '€', '\u{fffd}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{fffd}', 'Ž',
    '\u{fffd}', '\u{fffd}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{fffd}',
    'ž', 'Ÿ', '\u{a0}', '¡', '¢', '£', '¤', '¥', '¦', '§', '¨', '©', 'ª', '«', '¬', '\u{ad}', '®',
    '¯', '°', '±', '²', '³', '´', 'µ', '¶', '·', '¸', '¹', 'º', '»', '¼', '½', '¾', '¿', 'À', 'Á',
    'Â', 'Ã', 'Ä', 'Å', 'Æ', 'Ç', 'È', 'É', 'Ê', 'Ë', 'Ì', 'Í', 'Î', 'Ï', 'Ð', 'Ñ', 'Ò', 'Ó', 'Ô',
    'Õ', 'Ö', '×', 'Ø', 'Ù', 'Ú', 'Û', 'Ü', 'Ý', 'Þ', 'ß', 'à', 'á', 'â', 'ã', 'ä', 'å', 'æ', 'ç',
    'è', 'é', 'ê', 'ë', 'ì', 'í', 'î', 'ï', 'ð', 'ñ', 'ò', 'ó', 'ô', 'õ', 'ö', '÷', 'ø', 'ù', 'ú',
    'û', 'ü', 'ý', 'þ', 'ÿ',
];

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;
    use quireink_sdk::document::BlockKind;

    const BOOK: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
<description><title-info>
  <genre>sf</genre>
  <author><first-name>Ann</first-name><last-name>Example</last-name></author>
  <book-title>A   Test</book-title>
  <annotation><p>What it is</p><p>about.</p></annotation>
  <keywords>space, ships</keywords>
  <lang>en</lang>
  <coverpage><image l:href="#cover.png"/></coverpage>
  <sequence name="Tests" number="2"/>
</title-info></description>
<body>
  <title><p>A Test</p></title>
  <section><title><p>Chapter 1</p><p>The start</p></title>
    <epigraph><p>Words.</p><text-author>Someone</text-author></epigraph>
    <p>Some <emphasis>text</emphasis> and a note<a l:href="#n1" type="note">1</a>.</p>
    <empty-line/>
    <poem><stanza><v>Line one</v><v>Line two</v></stanza></poem>
    <section><title><p>Part 1.1</p></title><p>Inner.</p></section>
  </section>
</body>
<body name="notes"><title><p>Notes</p></title>
  <section id="n1"><title><p>1</p></title><p>The note.</p></section>
</body>
<binary id="cover.png" content-type="image/png">iVBORw0KGgo=</binary>
</FictionBook>"##;

    fn texts(doc: &Fb2Document, storage: &dyn Storage) -> Vec<(BlockKind, String)> {
        let mut out = Vec::new();
        let mut at = Locator::START;
        while let Some(block) = doc.block_at(storage, at).unwrap() {
            out.push((block.kind, block.text.clone()));
            at = Locator::new(0, block.end);
        }
        out
    }

    #[test]
    fn reads_metadata_text_contents_and_notes() {
        let storage = MemoryStorage::new().with("b/test.fb2", BOOK.as_bytes().to_vec());
        let doc = Fb2Document::open(&storage, "b/test.fb2", true).unwrap();
        let m = doc.metadata();
        assert_eq!(m.title.as_deref(), Some("A Test"));
        assert_eq!(m.authors, ["Ann Example"]);
        assert_eq!(m.language.as_deref(), Some("en"));
        assert_eq!(m.series.as_deref(), Some("Tests"));
        assert_eq!(m.series_index.as_deref(), Some("2"));
        assert_eq!(m.subjects, ["space", "ships"]);
        assert_eq!(m.description.as_deref(), Some("What it is about."));
        let blocks = texts(&doc, &storage);
        let only: Vec<&str> = blocks.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            only,
            [
                "A Test",
                "Chapter 1\nThe start",
                "Words.",
                "Someone",
                "Some text and a note1.",
                "Line one",
                "Line two",
                "Part 1.1",
                "Inner.",
                "Notes",
                "1",
                "The note."
            ]
        );
        assert_eq!(blocks[0].0, BlockKind::Heading(1));
        assert_eq!(blocks[1].0, BlockKind::Heading(2));
        assert_eq!(blocks[7].0, BlockKind::Heading(3));
        // The contents: the sections of the text, not the notes.
        assert_eq!(doc.toc_len(&storage), 2);
        let first = doc.toc_entry(&storage, 0).unwrap();
        assert_eq!(
            (first.title.as_str(), first.level),
            ("Chapter 1 The start", 0)
        );
        let second = doc.toc_entry(&storage, 1).unwrap();
        assert_eq!(second.level, 1);
        let block = doc.block_at(&storage, second.at).unwrap().unwrap();
        assert_eq!(block.text, "Part 1.1");
        // The note link leads to the note.
        let links = doc.links(&storage, 0, 0, u32::MAX);
        assert_eq!(links.len(), 1);
        let note = doc.link_target(&storage, &links[0].href).unwrap();
        let block = doc.block_at(&storage, note).unwrap().unwrap();
        assert_eq!(block.text, "1");
        // Opened again: from the cache.
        let again = Fb2Document::open(&storage, "b/test.fb2", true).unwrap();
        assert_eq!(again.metadata().authors, ["Ann Example"]);
        assert_eq!(again.toc_len(&storage), 2);
    }

    #[test]
    fn reads_windows_1251() {
        let mut book = Vec::new();
        book.extend_from_slice(
            b"<?xml version=\"1.0\" encoding=\"windows-1251\"?><FictionBook><description>\
              <title-info><book-title>",
        );
        // "Мир" in Windows-1251.
        book.extend_from_slice(&[0xCC, 0xE8, 0xF0]);
        book.extend_from_slice(b"</book-title></title-info></description><body><section><p>");
        book.extend_from_slice(&[0xCC, 0xE8, 0xF0]);
        book.extend_from_slice(b"</p></section></body></FictionBook>");
        let storage = MemoryStorage::new().with("r.fb2", book);
        let doc = Fb2Document::open(&storage, "r.fb2", true).unwrap();
        assert_eq!(doc.metadata().title.as_deref(), Some("Мир"));
        assert_eq!(texts(&doc, &storage)[0].1, "Мир");
    }

    #[test]
    fn base64_is_decoded_in_pieces() {
        let mut out = Vec::new();
        let mut b = Base64::default();
        b.feed("aGVsb", &mut out);
        b.feed("G8=\n", &mut out);
        assert_eq!(out, b"hello");
    }
}

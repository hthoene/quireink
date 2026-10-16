//! Reading an EPUB's package: container, OPF (metadata, manifest, spine)
//! and table of contents (EPUB 3 navigation document or EPUB 2 NCX).
//!
//! Every file is tokenized while it is inflated, never held in memory, and
//! the result is written to the book's cache on first open, so later opens
//! read only a few small files and memory does not grow with the book:
//!
//! - `spine.bin`: per chapter its ZIP directory record and uncompressed
//!   size (two u32).
//! - `toc.dat`: table of contents records (level u8, section u32, fragment
//!   length u16, title length u16, fragment, title); `toc.idx` holds the
//!   offset of each record as u32.
//! - `pages.txt`: the printed edition's pages (EPUB 3 `page-list`), as
//!   `section<TAB>fragment<TAB>label` lines in reading order.
//! - `styles.txt`: the rules of all the book's stylesheets (see `css`).
//! - `fonts.txt`: the embedded fonts: first `id<TAB>identifier`, then per
//!   font `family<TAB>bold<TAB>italic<TAB>obfuscation<TAB>path`.
//! - `package.ini`: cache version, book size, metadata and counts. Written
//!   last, so its presence marks a complete package cache.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::config;
use quireink_sdk::document::{DocumentError, Metadata};
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::zip::ZipArchive;

use crate::css;
use crate::obfuscation::Method;

/// Bump when the cache format or the conversion changes.
pub(crate) const CACHE_VERSION: u32 = 21;

/// A chapter: where it is in the archive and its uncompressed size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chapter {
    pub record: u32,
    pub size: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Package {
    pub metadata: Metadata,
    /// The spine in reading order.
    pub spine: Vec<Chapter>,
    pub toc_len: u32,
    /// Archive path of the cover image, if the package names one.
    pub cover: Option<String>,
}

/// A table of contents entry as stored in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawToc {
    pub title: String,
    pub level: u8,
    pub section: u32,
    /// Fragment identifier (`#id`), if any.
    pub fragment: Option<String>,
}

/// Size of a `toc.dat` record header.
const TOC_HEADER: usize = 9;
/// Cache files are appended in chunks of about this size.
const FLUSH: usize = 256;

fn path(cache: &str, name: &str) -> String {
    alloc::format!("{cache}/{name}")
}

pub(crate) fn error(e: quireink_sdk::zip::ZipError) -> DocumentError {
    match e {
        quireink_sdk::zip::ZipError::Storage(e) => DocumentError::Storage(e),
        quireink_sdk::zip::ZipError::OutOfMemory => DocumentError::OutOfMemory,
        other => DocumentError::Malformed(Cow::Owned(other.to_string())),
    }
}

fn malformed(message: &'static str) -> DocumentError {
    DocumentError::Malformed(Cow::Borrowed(message))
}

/// Tokenizes an archive entry while it is inflated.
fn tokenize(
    storage: &dyn Storage,
    archive: &ZipArchive,
    name: &str,
    handle: &mut dyn FnMut(Token<'_>),
) -> Result<(), DocumentError> {
    let entry = archive
        .entry(storage, name)
        .map_err(error)?
        .ok_or_else(|| DocumentError::Malformed(Cow::Owned(alloc::format!("missing {name}"))))?;
    let mut tokenizer = Tokenizer::new();
    archive
        .stream(storage, &entry, &mut |chunk| {
            tokenizer.feed(chunk, handle);
            true
        })
        .map_err(error)?;
    tokenizer.finish(handle);
    Ok(())
}

/// The directory part of an archive path, with trailing slash.
fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..=i])
}

/// Resolves `href` relative to the directory `base` into an archive path
/// and a fragment.
pub(crate) fn resolve(base: &str, href: &str) -> (String, Option<String>) {
    let (path, fragment) = match href.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode(f))),
        None => (href, None),
    };
    let joined = if path.is_empty() {
        String::from(base.trim_end_matches('/'))
    } else {
        alloc::format!("{base}{}", percent_decode(path))
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    (parts.join("/"), fragment)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Collapses whitespace in a title.
/// The most of a description kept, in bytes.
const MAX_DESCRIPTION: usize = 2000;

/// A description as plain text: Calibre and others write HTML into it
/// (escaped), so tags go and entities are decoded; cut at
/// [`MAX_DESCRIPTION`].
fn plain_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                // Paragraphs and line breaks part words.
                text.push(' ');
            }
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let mut plain = clean(&decode(&text));
    if plain.len() > MAX_DESCRIPTION {
        let mut end = MAX_DESCRIPTION;
        while !plain.is_char_boundary(end) {
            end -= 1;
        }
        plain.truncate(end);
        plain.push('…');
    }
    plain
}

fn clean(text: &str) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// FNV-1a, to remember manifest ids without keeping them.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn read_u32(storage: &dyn Storage, path: &str, offset: u64) -> Option<u32> {
    let mut buf = [0u8; 4];
    (storage.read_at(path, offset, &mut buf).ok()? == 4).then(|| u32::from_le_bytes(buf))
}

/// The package from the cache, if it is complete and matches the book.
pub(crate) fn load(storage: &dyn Storage, cache: &str, book_size: u64) -> Option<Package> {
    let data = quireink_sdk::read_file(storage, &path(cache, "package.ini"), 16 * 1024).ok()?;
    let source = core::str::from_utf8(&data).ok()?;
    let mut metadata = Metadata::default();
    let (mut version, mut size, mut sections, mut toc_len) = (None, None, None, None);
    let mut cover = None;
    for entry in config::parse(source) {
        let entry = entry.ok()?;
        match (entry.section, entry.key) {
            ("cache", "version") => version = entry.value.as_u32(),
            ("cache", "size") => size = entry.value.raw().parse::<u64>().ok(),
            ("book", "title") => metadata.title = Some(entry.value.as_str().into_owned()),
            ("book", key) if key.starts_with("author.") => {
                metadata.authors.push(entry.value.as_str().into_owned());
            }
            ("book", "language") => metadata.language = Some(entry.value.as_str().into_owned()),
            ("book", "series") => metadata.series = Some(entry.value.as_str().into_owned()),
            ("book", "series-index") => {
                metadata.series_index = Some(entry.value.as_str().into_owned());
            }
            ("book", key) if key.starts_with("subject.") => {
                metadata.subjects.push(entry.value.as_str().into_owned());
            }
            ("book", "description") => {
                metadata.description = Some(entry.value.as_str().into_owned());
            }
            ("book", "sections") => sections = entry.value.as_u32(),
            ("book", "toc") => toc_len = entry.value.as_u32(),
            ("book", "cover") => cover = Some(entry.value.as_str().into_owned()),
            _ => {}
        }
    }
    if version != Some(CACHE_VERSION) || size != Some(book_size) {
        return None;
    }
    let sections = sections? as usize;
    let data = quireink_sdk::read_file(storage, &path(cache, "spine.bin"), sections * 8).ok()?;
    if data.len() != sections * 8 || sections == 0 {
        return None;
    }
    let spine = data
        .chunks_exact(8)
        .map(|c| Chapter {
            record: u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            size: u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
        })
        .collect();
    Some(Package {
        metadata,
        spine,
        toc_len: toc_len?,
        cover,
    })
}

/// Writes `fonts.txt`: the fonts the stylesheets embed and how
/// `META-INF/encryption.xml` says they are obfuscated.
fn write_fonts(
    storage: &dyn Storage,
    archive: &ZipArchive,
    cache: &str,
    faces: &[css::FontFace],
    identifier: &str,
) -> Result<(), DocumentError> {
    let mut methods: Vec<(String, Method)> = Vec::new();
    if !faces.is_empty() {
        let mut method = Method::None;
        // A missing encryption.xml is fine: nothing is obfuscated.
        let _ = tokenize(storage, archive, "META-INF/encryption.xml", &mut |token| {
            if let Token::Start { name, attrs, .. } = token {
                match local_name(name) {
                    "EncryptionMethod" => {
                        method =
                            Method::from_algorithm(&attrs.get("Algorithm").unwrap_or_default());
                    }
                    "CipherReference" => {
                        if let Some(uri) = attrs.get("URI") {
                            methods.push((resolve("", &uri).0, method));
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    let mut out = alloc::format!("id\t{}\n", identifier.replace(['\t', '\n'], " "));
    for face in faces {
        let method = methods
            .iter()
            .find(|(p, _)| *p == face.path)
            .map_or(Method::None, |(_, m)| *m);
        out.push_str(&alloc::format!(
            "{}\t{}\t{}\t{}\t{}\n",
            face.family,
            u8::from(face.bold),
            u8::from(face.italic),
            method.code(),
            face.path
        ));
    }
    storage.write(&path(cache, "fonts.txt"), out.as_bytes())?;
    Ok(())
}

/// An embedded font as listed in `fonts.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FontEntry {
    pub family: u8,
    pub bold: bool,
    pub italic: bool,
    pub method: Method,
    pub path: String,
}

/// The book's identifier and embedded fonts from the cache.
pub(crate) fn fonts(storage: &dyn Storage, cache: &str) -> (String, Vec<FontEntry>) {
    let Ok(data) = quireink_sdk::read_file(storage, &path(cache, "fonts.txt"), 64 * 1024) else {
        return (String::new(), Vec::new());
    };
    let text = String::from_utf8_lossy(&data);
    let mut identifier = String::new();
    let mut entries = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["id", id] => identifier = String::from(*id),
            [family, bold, italic, method, path] => {
                let Ok(family) = family.parse() else {
                    continue;
                };
                entries.push(FontEntry {
                    family,
                    bold: *bold == "1",
                    italic: *italic == "1",
                    method: Method::from_code(method.parse().unwrap_or(0)),
                    path: String::from(*path),
                });
            }
            _ => {}
        }
    }
    (identifier, entries)
}

/// The book's stylesheet rules from the cache.
pub(crate) fn styles(storage: &dyn Storage, cache: &str) -> css::Rules {
    quireink_sdk::read_file(storage, &path(cache, "styles.txt"), 64 * 1024)
        .ok()
        .and_then(|data| String::from_utf8(data).ok())
        .map(|text| css::Rules::decode(&text))
        .unwrap_or_default()
}

/// Removes the files that mark a complete cache, so it is rebuilt.
pub(crate) fn invalidate(
    storage: &dyn Storage,
    cache: &str,
    chapter_marker: impl Fn(u32) -> String,
) {
    let spine = path(cache, "spine.bin");
    let Ok(size) = storage.size(&spine) else {
        // A cache from before there was a spine file: no telling which of
        // its files mark chapters, so all go.
        let _ = storage.remove_dir_all(cache);
        return;
    };
    let sections = size / 8;
    for section in 0..sections as u32 {
        let _ = storage.remove(&chapter_marker(section));
    }
    let _ = storage.remove(&path(cache, "package.ini"));
}

/// Reads table of contents entry `index` from the cache.
pub(crate) fn toc_entry(storage: &dyn Storage, cache: &str, index: u32) -> Option<RawToc> {
    let data = path(cache, "toc.dat");
    let offset = u64::from(read_u32(
        storage,
        &path(cache, "toc.idx"),
        u64::from(index) * 4,
    )?);
    let mut header = [0u8; TOC_HEADER];
    if storage.read_at(&data, offset, &mut header).ok()? != TOC_HEADER {
        return None;
    }
    let section = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
    let fragment_len = usize::from(u16::from_le_bytes([header[5], header[6]]));
    let title_len = usize::from(u16::from_le_bytes([header[7], header[8]]));
    let mut text = alloc::vec![0u8; fragment_len + title_len];
    let mut filled = 0;
    while filled < text.len() {
        let n = storage
            .read_at(
                &data,
                offset + (TOC_HEADER + filled) as u64,
                &mut text[filled..],
            )
            .ok()?;
        if n == 0 {
            return None;
        }
        filled += n;
    }
    let (fragment, title) = text.split_at(fragment_len);
    Some(RawToc {
        title: String::from_utf8_lossy(title).into_owned(),
        level: header[0],
        section,
        fragment: (fragment_len > 0).then(|| String::from_utf8_lossy(fragment).into_owned()),
    })
}

/// `text` cut to at most `max` bytes, at a character boundary.
fn truncate(text: &str, max: usize) -> &str {
    let mut cut = text.len().min(max);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    &text[..cut]
}

/// Writes table of contents records to the cache in small chunks.
struct TocWriter<'a> {
    storage: &'a dyn Storage,
    data_path: String,
    index_path: String,
    data: Vec<u8>,
    index: Vec<u8>,
    written: u32,
    count: u32,
    failed: Option<DocumentError>,
}

impl<'a> TocWriter<'a> {
    fn new(storage: &'a dyn Storage, cache: &str) -> Result<Self, DocumentError> {
        let writer = Self {
            storage,
            data_path: path(cache, "toc.dat"),
            index_path: path(cache, "toc.idx"),
            data: Vec::new(),
            index: Vec::new(),
            written: 0,
            count: 0,
            failed: None,
        };
        storage.write(&writer.data_path, &[])?;
        storage.write(&writer.index_path, &[])?;
        Ok(writer)
    }

    fn push(&mut self, level: u8, section: u32, fragment: Option<&str>, title: &str) {
        // Longer ids cannot match an anchor; titles are shown on one line.
        let fragment = truncate(fragment.unwrap_or_default(), 255);
        let title = truncate(title, 1024);
        self.index.extend_from_slice(&self.written.to_le_bytes());
        self.data.push(level);
        self.data.extend_from_slice(&section.to_le_bytes());
        self.data
            .extend_from_slice(&(fragment.len() as u16).to_le_bytes());
        self.data
            .extend_from_slice(&(title.len() as u16).to_le_bytes());
        self.data.extend_from_slice(fragment.as_bytes());
        self.data.extend_from_slice(title.as_bytes());
        self.written += (TOC_HEADER + fragment.len() + title.len()) as u32;
        self.count += 1;
        if self.data.len() >= FLUSH || self.index.len() >= FLUSH {
            self.flush();
        }
    }

    fn flush(&mut self) {
        for (path, buffer) in [
            (&self.data_path, &mut self.data),
            (&self.index_path, &mut self.index),
        ] {
            if !buffer.is_empty()
                && self.failed.is_none()
                && let Err(e) = self.storage.append(path, buffer)
            {
                self.failed = Some(DocumentError::Storage(e));
            }
            buffer.clear();
        }
    }

    fn finish(mut self) -> Result<u32, DocumentError> {
        self.flush();
        match self.failed {
            Some(e) => Err(e),
            None => Ok(self.count),
        }
    }
}

/// Reads the package from the archive and writes it to the cache.
pub(crate) fn build(
    storage: &dyn Storage,
    archive: &ZipArchive,
    cache: &str,
    book_size: u64,
) -> Result<Package, DocumentError> {
    // container.xml → path of the OPF package document.
    let mut opf_path = None;
    tokenize(storage, archive, "META-INF/container.xml", &mut |token| {
        if let Token::Start { name, attrs, .. } = token
            && local_name(name) == "rootfile"
            && opf_path.is_none()
        {
            opf_path = attrs.get("full-path").map(|p| p.into_owned());
        }
    })?;
    let opf_path = opf_path.ok_or(malformed("no package document"))?;
    let opf_dir = String::from(dir_of(&opf_path));

    // First pass: metadata and the spine's ids (hashed: 8 bytes each).
    let mut metadata = Metadata::default();
    let mut spine_ids: Vec<(u64, u32)> = Vec::new();
    let mut ncx_id: Option<u64> = None;
    // EPUB 2 names the cover's manifest item in `<meta name="cover">`.
    let mut cover_id: Option<u64> = None;
    let mut capture: Option<&'static str> = None;
    let mut text = String::new();
    // The unique identifier, the key of obfuscated fonts.
    let mut unique_id: Option<String> = None;
    let mut identifier = String::new();
    // EPUB 3 series: `<meta property="belongs-to-collection" id="c">`, its
    // number in `<meta refines="#c" property="group-position">`.
    let mut collection_id: Option<String> = None;
    tokenize(storage, archive, &opf_path, &mut |token| match token {
        Token::Start {
            name,
            attrs,
            self_closing,
        } => match local_name(name) {
            "package" => unique_id = attrs.get("unique-identifier").map(|v| v.into_owned()),
            "identifier"
                if !self_closing
                    && identifier.is_empty()
                    && attrs
                        .get("id")
                        .is_some_and(|id| unique_id.as_deref() == Some(&*id)) =>
            {
                capture = Some("identifier");
                text.clear();
            }
            "title" | "creator" | "language" | "subject" | "description" if !self_closing => {
                capture = match local_name(name) {
                    "title" => Some("title"),
                    "creator" => Some("creator"),
                    "subject" => Some("subject"),
                    "description" => Some("description"),
                    _ => Some("language"),
                };
                text.clear();
            }
            "spine" => ncx_id = attrs.get("toc").map(|v| hash(&v)),
            "meta" if attrs.get("name").as_deref() == Some("cover") => {
                cover_id = attrs.get("content").map(|v| hash(&v));
            }
            // Calibre's series.
            "meta" if attrs.get("name").as_deref() == Some("calibre:series") => {
                if let Some(series) = attrs.get("content").map(|v| clean(&v)) {
                    metadata.series.get_or_insert(series);
                }
            }
            "meta" if attrs.get("name").as_deref() == Some("calibre:series_index") => {
                if let Some(index) = attrs.get("content").map(|v| clean(&v)) {
                    metadata.series_index.get_or_insert(index);
                }
            }
            "meta" if !self_closing => {
                let property = attrs.get("property");
                if property.as_deref() == Some("belongs-to-collection") {
                    collection_id = attrs.get("id").map(|v| v.into_owned());
                    capture = Some("collection");
                    text.clear();
                } else if property.as_deref() == Some("group-position")
                    && attrs.get("refines").is_some_and(|r| {
                        collection_id
                            .as_deref()
                            .is_some_and(|id| r.trim_start_matches('#') == id)
                    })
                {
                    capture = Some("position");
                    text.clear();
                }
            }
            "itemref" => {
                if let Some(id) = attrs.get("idref") {
                    let position = spine_ids.len() as u32;
                    spine_ids.push((hash(&id), position));
                }
            }
            _ => {}
        },
        Token::Text(t) if capture.is_some() => text.push_str(&decode(&t)),
        Token::CData(t) if capture.is_some() => text.push_str(t),
        Token::End { name } => {
            // Series are captured from `<meta>` elements.
            let element = |field: &'static str| -> &'static str {
                match field {
                    "collection" | "position" => "meta",
                    field => field,
                }
            };
            if let Some(field) = capture
                && local_name(name) == element(field)
            {
                let value = clean(&text);
                match field {
                    "title" if metadata.title.is_none() && !value.is_empty() => {
                        metadata.title = Some(value);
                    }
                    "creator" if !value.is_empty() => metadata.authors.push(value),
                    "subject" if !value.is_empty() && !metadata.subjects.contains(&value) => {
                        metadata.subjects.push(value);
                    }
                    "description" if metadata.description.is_none() => {
                        let plain = plain_text(&value);
                        metadata.description = (!plain.is_empty()).then_some(plain);
                    }
                    "language" if metadata.language.is_none() && !value.is_empty() => {
                        metadata.language = Some(value);
                    }
                    "identifier" => identifier = value,
                    "collection" if !value.is_empty() => {
                        metadata.series.get_or_insert(value);
                    }
                    "position" if !value.is_empty() => {
                        metadata.series_index.get_or_insert(value);
                    }
                    _ => {}
                }
                capture = None;
            }
        }
        _ => {}
    })?;
    spine_ids.sort_unstable();

    // Second pass: the manifest items the spine refers to, the navigation
    // document and the NCX. Nothing else of the manifest is kept.
    let mut chapters: Vec<Option<Chapter>> = alloc::vec![None; spine_ids.len()];
    let mut nav: Option<String> = None;
    let mut ncx: Option<String> = None;
    let mut ncx_by_type: Option<String> = None;
    let mut stylesheets: Vec<String> = Vec::new();
    // EPUB 3 `cover-image` wins over EPUB 2's `<meta name="cover">`.
    let mut cover: Option<(bool, String)> = None;
    let mut failed = None;
    tokenize(storage, archive, &opf_path, &mut |token| {
        let Token::Start { name, attrs, .. } = token else {
            return;
        };
        if local_name(name) != "item" {
            return;
        }
        let get = |n| attrs.get(n).unwrap_or_default();
        let id = hash(&get("id"));
        let first = spine_ids.partition_point(|&(h, _)| h < id);
        let positions = spine_ids[first..].iter().take_while(|&&(h, _)| h == id);
        let is_nav = get("properties").split_whitespace().any(|p| p == "nav");
        let is_ncx = ncx_id == Some(id);
        let media_type = get("media-type");
        let is_ncx_type = media_type == "application/x-dtbncx+xml";
        let is_css = media_type == "text/css";
        let is_cover_image = get("properties")
            .split_whitespace()
            .any(|p| p == "cover-image");
        if (is_cover_image || cover_id == Some(id))
            && media_type.starts_with("image/")
            && !cover.as_ref().is_some_and(|(declared, _)| *declared)
        {
            cover = Some((is_cover_image, resolve(&opf_dir, &get("href")).0));
        }
        let mut positions = positions.peekable();
        if positions.peek().is_none() && !is_nav && !is_ncx && !is_ncx_type && !is_css {
            return;
        }
        let (path, _) = resolve(&opf_dir, &get("href"));
        if is_css {
            stylesheets.push(path.clone());
        }
        for &(_, position) in positions {
            match archive.entry(storage, &path) {
                Ok(Some(entry)) => {
                    chapters[position as usize] = Some(Chapter {
                        record: entry.record(),
                        size: entry.size,
                    });
                }
                Ok(None) => {}
                Err(e) => failed = Some(error(e)),
            }
        }
        if is_nav && nav.is_none() {
            nav = Some(path.clone());
        }
        if is_ncx {
            ncx = Some(path.clone());
        }
        if is_ncx_type && ncx_by_type.is_none() {
            ncx_by_type = Some(path);
        }
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    drop(spine_ids);
    let spine: Vec<Chapter> = chapters.into_iter().flatten().collect();
    if spine.is_empty() {
        return Err(malformed("empty spine"));
    }

    // Table of contents: EPUB 3 navigation document, else (or if it has no
    // entries) the EPUB 2 NCX.
    let mut toc = TocWriter::new(storage, cache)?;
    let add =
        |toc: &mut TocWriter<'_>, title: String, level: u8, file: &str, fragment: Option<&str>| {
            let Ok(Some(entry)) = archive.entry(storage, file) else {
                return;
            };
            if let Some(section) = spine.iter().position(|c| c.record == entry.record()) {
                toc.push(level, section as u32, fragment, &title);
            }
        };
    let pages_path = path(cache, "pages.txt");
    storage.write(&pages_path, &[])?;
    if let Some(nav) = &nav {
        let mut pages = String::new();
        let mut add_page = |label: String, file: &str, fragment: Option<&str>| {
            let (Ok(Some(entry)), Some(fragment)) = (archive.entry(storage, file), fragment) else {
                return;
            };
            if let Some(section) = spine.iter().position(|c| c.record == entry.record()) {
                let one = |s: &str| s.replace(['\t', '\n'], " ");
                pages.push_str(&alloc::format!(
                    "{section}\t{}\t{}\n",
                    one(fragment),
                    one(&label)
                ));
                if pages.len() >= 1024 {
                    let _ = storage.append(&pages_path, pages.as_bytes());
                    pages.clear();
                }
            }
        };
        let _ = read_nav(
            storage,
            archive,
            nav,
            &mut |t, l, f, fr| add(&mut toc, t, l, f, fr),
            &mut add_page,
        );
        let _ = storage.append(&pages_path, pages.as_bytes());
    }
    let ncx = ncx.or(ncx_by_type);
    if let Some(ncx) = &ncx
        && toc.count == 0
    {
        let _ = read_ncx(storage, archive, ncx, &mut |t, l, f, fr| {
            add(&mut toc, t, l, f, fr)
        });
    }
    let toc_len = toc.finish()?;

    let mut spine_data = Vec::with_capacity(spine.len() * 8);
    for chapter in &spine {
        spine_data.extend_from_slice(&chapter.record.to_le_bytes());
        spine_data.extend_from_slice(&chapter.size.to_le_bytes());
    }
    storage.write(&path(cache, "spine.bin"), &spine_data)?;
    drop(spine_data);

    // Stylesheets, one at a time; a damaged one is skipped.
    let mut rules = css::Rules::default();
    for path in &stylesheets {
        let mut parser = css::Parser::new(dir_of(path));
        let Ok(Some(entry)) = archive.entry(storage, path) else {
            continue;
        };
        let mut pending: Vec<u8> = Vec::new();
        let _ = archive.stream(storage, &entry, &mut |chunk| {
            // Chunks may end inside a UTF-8 sequence.
            pending.extend_from_slice(chunk);
            let valid = match core::str::from_utf8(&pending) {
                Ok(text) => text.len(),
                Err(e) => e.valid_up_to(),
            };
            if let Ok(text) = core::str::from_utf8(&pending[..valid]) {
                parser.feed(text, &mut rules);
            }
            pending.drain(..valid);
            true
        });
    }
    storage.write(&path(cache, "styles.txt"), rules.encode().as_bytes())?;
    write_fonts(storage, archive, cache, &rules.faces, &identifier)?;
    drop(rules);

    let mut ini = String::new();
    let mut set = |section: &str, key: &str, value: &str| {
        ini = config::set(&ini, section, key, value);
    };
    set("cache", "version", &CACHE_VERSION.to_string());
    set("cache", "size", &book_size.to_string());
    if let Some(title) = &metadata.title {
        set("book", "title", title);
    }
    if let Some(language) = &metadata.language {
        set("book", "language", language);
    }
    if let Some(series) = &metadata.series {
        set("book", "series", series);
    }
    if let Some(index) = &metadata.series_index {
        set("book", "series-index", index);
    }
    set("book", "sections", &spine.len().to_string());
    set("book", "toc", &toc_len.to_string());
    for (i, author) in metadata.authors.iter().enumerate() {
        set("book", &alloc::format!("author.{}", i + 1), author);
    }
    for (i, subject) in metadata.subjects.iter().enumerate() {
        set("book", &alloc::format!("subject.{}", i + 1), subject);
    }
    if let Some(description) = &metadata.description {
        set("book", "description", description);
    }
    let cover = cover.map(|(_, path)| path);
    if let Some(cover) = &cover {
        set("book", "cover", cover);
    }
    storage.write(&path(cache, "package.ini"), ini.as_bytes())?;

    Ok(Package {
        metadata,
        spine,
        toc_len,
        cover,
    })
}

type AddToc<'a> = dyn FnMut(String, u8, &str, Option<&str>) + 'a;

/// EPUB 3: `<nav epub:type="toc">` with nested `<ol><li><a href>`, and
/// the printed pages of `<nav epub:type="page-list">` for `page`.
fn read_nav(
    storage: &dyn Storage,
    archive: &ZipArchive,
    path: &str,
    add: &mut AddToc<'_>,
    page: &mut dyn FnMut(String, &str, Option<&str>),
) -> Result<(), DocumentError> {
    let base = String::from(dir_of(path));
    let mut in_toc = false;
    let mut in_pages = false;
    let mut page_link: Option<(String, Option<String>)> = None;
    let mut ol_depth = 0u8;
    let mut link: Option<(String, Option<String>)> = None;
    let mut text = String::new();
    tokenize(storage, archive, path, &mut |token| match token {
        Token::Start {
            name,
            attrs,
            self_closing,
        } => match local_name(name) {
            "nav" => {
                let kind = attrs
                    .iter()
                    .find(|(n, _)| local_name(n) == "type")
                    .map(|(_, v)| v);
                if kind.is_some_and(|k| k.split_whitespace().any(|t| t == "toc")) {
                    in_toc = true;
                    ol_depth = 0;
                }
                in_pages = kind.is_some_and(|k| k.split_whitespace().any(|t| t == "page-list"));
            }
            "a" if in_pages && !self_closing => {
                let href = attrs.get("href").unwrap_or_default();
                page_link = Some(resolve(&base, &href));
                text.clear();
            }
            "ol" if in_toc => ol_depth += 1,
            "a" if in_toc && !self_closing => {
                let href = attrs.get("href").unwrap_or_default();
                link = Some(resolve(&base, &href));
                text.clear();
            }
            _ => {}
        },
        Token::Text(t) if link.is_some() || page_link.is_some() => text.push_str(&decode(&t)),
        Token::End { name } => match local_name(name) {
            "nav" => {
                in_toc = false;
                in_pages = false;
            }
            "a" if page_link.is_some() => {
                if let Some((file, fragment)) = page_link.take() {
                    let label = clean(&text);
                    if !label.is_empty() && !file.is_empty() {
                        page(label, &file, fragment.as_deref());
                    }
                }
            }
            "ol" if in_toc => ol_depth = ol_depth.saturating_sub(1),
            "a" => {
                if let Some((file, fragment)) = link.take() {
                    let title = clean(&text);
                    if !title.is_empty() && !file.is_empty() {
                        add(
                            title,
                            ol_depth.saturating_sub(1),
                            &file,
                            fragment.as_deref(),
                        );
                    }
                }
            }
            _ => {}
        },
        _ => {}
    })
}

/// EPUB 2: nested `<navPoint>` with `<navLabel><text>` and `<content src>`
/// inside `<navMap>` (page lists and other lists are not chapters).
fn read_ncx(
    storage: &dyn Storage,
    archive: &ZipArchive,
    path: &str,
    add: &mut AddToc<'_>,
) -> Result<(), DocumentError> {
    let base = String::from(dir_of(path));
    let mut depth = 0u8;
    let mut in_map = false;
    let mut in_label = false;
    let mut text = String::new();
    let mut pending: Option<(String, u8)> = None;
    tokenize(storage, archive, path, &mut |token| match token {
        Token::Start { name, attrs, .. } => match local_name(name) {
            "navMap" => in_map = true,
            _ if !in_map => {}
            "navPoint" => depth = depth.saturating_add(1),
            "navLabel" => {
                in_label = true;
                text.clear();
            }
            "content" => {
                if let (Some((title, level)), Some(src)) = (pending.take(), attrs.get("src")) {
                    let (file, fragment) = resolve(&base, &src);
                    add(title, level, &file, fragment.as_deref());
                }
            }
            _ => {}
        },
        Token::Text(t) if in_label => text.push_str(&decode(&t)),
        Token::End { name } => match local_name(name) {
            "navMap" => in_map = false,
            "navPoint" => depth = depth.saturating_sub(1),
            "navLabel" if in_map => {
                in_label = false;
                let title = clean(&text);
                if !title.is_empty() {
                    pending = Some((title, depth.saturating_sub(1)));
                }
            }
            _ => {}
        },
        _ => {}
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_paths() {
        assert_eq!(
            resolve("OEBPS/", "text/ch1.xhtml"),
            ("OEBPS/text/ch1.xhtml".into(), None)
        );
        assert_eq!(
            resolve("OEBPS/text/", "../Text/ch%202.xhtml#sec-1"),
            ("OEBPS/Text/ch 2.xhtml".into(), Some("sec-1".into()))
        );
        assert_eq!(resolve("", "./a.xhtml"), ("a.xhtml".into(), None));
    }
}

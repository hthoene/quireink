//! Reading an EPUB's package: container, OPF (metadata, manifest, spine)
//! and table of contents (EPUB 3 navigation document or EPUB 2 NCX).
//! Every file is tokenized while it is inflated, never held in memory.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::document::{DocumentError, Metadata};
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::zip::ZipArchive;

/// A table of contents entry before it is resolved to a locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawToc {
    pub title: String,
    pub level: u8,
    /// Archive path of the target file.
    pub file: String,
    /// Fragment identifier (`#id`), if any.
    pub fragment: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Package {
    pub metadata: Metadata,
    /// Archive paths of the spine documents in reading order.
    pub spine: Vec<String>,
    pub toc: Vec<RawToc>,
}

pub(crate) fn error(e: quireink_sdk::zip::ZipError) -> DocumentError {
    match e {
        quireink_sdk::zip::ZipError::Storage(e) => DocumentError::Storage(e),
        quireink_sdk::zip::ZipError::OutOfMemory => DocumentError::OutOfMemory,
        other => DocumentError::Malformed(Cow::Owned(other.to_string())),
    }
}

/// Tokenizes an archive entry while it is inflated.
fn tokenize(
    storage: &dyn Storage,
    archive: &ZipArchive,
    name: &str,
    handle: &mut dyn FnMut(Token<'_>),
) -> Result<(), DocumentError> {
    let entry = archive
        .entry(name)
        .ok_or_else(|| DocumentError::Malformed(Cow::Owned(alloc::format!("missing {name}"))))?;
    let mut tokenizer = Tokenizer::new();
    archive
        .stream(storage, entry, &mut |chunk| {
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

pub(crate) fn read(storage: &dyn Storage, archive: &ZipArchive) -> Result<Package, DocumentError> {
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
    let opf_path = opf_path.ok_or(DocumentError::Malformed(Cow::Borrowed(
        "no package document",
    )))?;
    let opf_dir = String::from(dir_of(&opf_path));

    // The package document.
    let mut metadata = Metadata::default();
    // Only what is needed later: id → path, the navigation document and
    // an NCX found by its media type.
    let mut manifest: Vec<(String, String)> = Vec::new();
    let mut nav: Option<String> = None;
    let mut ncx_by_type: Option<String> = None;
    let mut spine_ids: Vec<String> = Vec::new();
    let mut ncx_id: Option<String> = None;
    let mut capture: Option<&'static str> = None;
    let mut text = String::new();
    tokenize(storage, archive, &opf_path, &mut |token| match token {
        Token::Start {
            name,
            attrs,
            self_closing,
        } => match local_name(name) {
            "title" | "creator" | "language" if !self_closing => {
                capture = match local_name(name) {
                    "title" => Some("title"),
                    "creator" => Some("creator"),
                    _ => Some("language"),
                };
                text.clear();
            }
            "item" => {
                let get = |n| attrs.get(n).map(|v| v.into_owned()).unwrap_or_default();
                let (path, _) = resolve(&opf_dir, &get("href"));
                if nav.is_none() && get("properties").split_whitespace().any(|p| p == "nav") {
                    nav = Some(path.clone());
                }
                if ncx_by_type.is_none() && get("media-type") == "application/x-dtbncx+xml" {
                    ncx_by_type = Some(path.clone());
                }
                manifest.push((get("id"), path));
            }
            "spine" => ncx_id = attrs.get("toc").map(|v| v.into_owned()),
            "itemref" => {
                if let Some(id) = attrs.get("idref") {
                    spine_ids.push(id.into_owned());
                }
            }
            _ => {}
        },
        Token::Text(t) if capture.is_some() => text.push_str(&decode(&t)),
        Token::CData(t) if capture.is_some() => text.push_str(t),
        Token::End { name } => {
            if let Some(field) = capture
                && local_name(name) == field
            {
                let value = clean(&text);
                match field {
                    "title" if metadata.title.is_none() && !value.is_empty() => {
                        metadata.title = Some(value);
                    }
                    "creator" if !value.is_empty() => metadata.authors.push(value),
                    "language" if metadata.language.is_none() && !value.is_empty() => {
                        metadata.language = Some(value);
                    }
                    _ => {}
                }
                capture = None;
            }
        }
        _ => {}
    })?;

    let path_of = |id: &str| {
        manifest
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, path)| path.clone())
    };
    let spine: Vec<String> = spine_ids
        .iter()
        .filter_map(|id| path_of(id))
        .filter(|path| archive.entry(path).is_some())
        .collect();
    if spine.is_empty() {
        return Err(DocumentError::Malformed(Cow::Borrowed("empty spine")));
    }

    // Table of contents: EPUB 3 navigation document, else EPUB 2 NCX.
    let ncx = ncx_id.as_deref().and_then(path_of).or(ncx_by_type);
    // Free the manifest before the table of contents is read.
    drop(manifest);
    drop(spine_ids);
    let toc = nav
        .and_then(|nav| read_nav(storage, archive, &nav).ok())
        .unwrap_or_default();
    // An NCX if there is no navigation document or it had no entries.
    let toc = match ncx {
        Some(ncx) if toc.is_empty() => read_ncx(storage, archive, &ncx).unwrap_or_default(),
        _ => toc,
    };

    Ok(Package {
        metadata,
        spine,
        toc,
    })
}

/// EPUB 3: `<nav epub:type="toc">` with nested `<ol><li><a href>`.
fn read_nav(
    storage: &dyn Storage,
    archive: &ZipArchive,
    path: &str,
) -> Result<Vec<RawToc>, DocumentError> {
    let base = String::from(dir_of(path));
    let mut entries = Vec::new();
    let mut in_toc = false;
    let mut nav_depth = 0u32;
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
                nav_depth += 1;
                let kind = attrs
                    .iter()
                    .find(|(n, _)| local_name(n) == "type")
                    .map(|(_, v)| v);
                if kind.is_some_and(|k| k.split_whitespace().any(|t| t == "toc")) {
                    in_toc = true;
                    ol_depth = 0;
                }
            }
            "ol" if in_toc => ol_depth += 1,
            "a" if in_toc && !self_closing => {
                let href = attrs
                    .get("href")
                    .map(|h| h.into_owned())
                    .unwrap_or_default();
                link = Some(resolve(&base, &href));
                text.clear();
            }
            _ => {}
        },
        Token::Text(t) if link.is_some() => text.push_str(&decode(&t)),
        Token::End { name } => match local_name(name) {
            "nav" => {
                nav_depth = nav_depth.saturating_sub(1);
                in_toc = false;
            }
            "ol" if in_toc => ol_depth = ol_depth.saturating_sub(1),
            "a" => {
                if let Some((file, fragment)) = link.take() {
                    let title = clean(&text);
                    if !title.is_empty() && !file.is_empty() {
                        entries.push(RawToc {
                            title,
                            level: ol_depth.saturating_sub(1),
                            file,
                            fragment,
                        });
                    }
                }
            }
            _ => {}
        },
        _ => {}
    })?;
    let _ = nav_depth;
    Ok(entries)
}

/// EPUB 2: nested `<navPoint>` with `<navLabel><text>` and `<content src>`.
fn read_ncx(
    storage: &dyn Storage,
    archive: &ZipArchive,
    path: &str,
) -> Result<Vec<RawToc>, DocumentError> {
    let base = String::from(dir_of(path));
    let mut entries: Vec<RawToc> = Vec::new();
    let mut depth = 0u8;
    let mut in_label = false;
    let mut text = String::new();
    let mut pending: Option<(String, u8)> = None;
    tokenize(storage, archive, path, &mut |token| match token {
        Token::Start { name, attrs, .. } => match local_name(name) {
            "navPoint" => depth = depth.saturating_add(1),
            "navLabel" => {
                in_label = true;
                text.clear();
            }
            "content" => {
                if let (Some((title, level)), Some(src)) = (pending.take(), attrs.get("src")) {
                    let (file, fragment) = resolve(&base, &src);
                    entries.push(RawToc {
                        title,
                        level,
                        file,
                        fragment,
                    });
                }
            }
            _ => {}
        },
        Token::Text(t) if in_label => text.push_str(&decode(&t)),
        Token::End { name } => match local_name(name) {
            "navPoint" => depth = depth.saturating_sub(1),
            "navLabel" => {
                in_label = false;
                let title = clean(&text);
                if !title.is_empty() {
                    pending = Some((title, depth.saturating_sub(1)));
                }
            }
            _ => {}
        },
        _ => {}
    })?;
    Ok(entries)
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

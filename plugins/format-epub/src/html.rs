//! HTML files (web pages, saved or downloaded) as documents of one
//! section, converted like EPUB chapters (see [`crate::convert`]) and read
//! as browsers read HTML: only the reading matter, links made absolute
//! with the page's address (from its `<base href>`).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::document::{Block, Document, DocumentError, Link, Locator, Metadata};
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;
use crate::cached;
use crate::convert::{self, CachePaths, INDEX, Source};

/// Changes whenever pages are read differently (e.g. what counts as
/// page furniture), so converted pages are converted again.
const HTML_VERSION: u32 = 2;

/// Marks a page whose converted length is not known yet.
const UNKNOWN: u32 = u32::MAX;
/// Longest title kept.
const TITLE: usize = 300;

/// An open HTML file.
#[derive(Debug)]
pub struct HtmlDocument {
    path: String,
    paths: CachePaths,
    metadata: Metadata,
    /// The element holding the reading matter, if the page marks it.
    focus: Option<&'static str>,
    /// The page's address.
    base: Option<String>,
    styles: bool,
    /// Size of the file: the length until it is converted.
    size: u32,
    length: Cell<u32>,
}

/// What a first look at a page finds.
#[derive(Debug, Default)]
struct Scan {
    title: Option<String>,
    language: Option<String>,
    focus: Option<&'static str>,
    base: Option<String>,
}

/// Reads the title, language, address and reading matter of a page.
fn scan(storage: &dyn Storage, path: &str) -> Result<Scan, DocumentError> {
    let mut found = Scan::default();
    let (mut main, mut article) = (false, false);
    let mut title: Option<String> = None;
    let mut handle = |token: Token<'_>| match token {
        Token::Start { name, attrs, .. } => match local_name(name).to_ascii_lowercase().as_str() {
            "html" => found.language = attrs.get("lang").map(|l| l.trim().to_string()),
            "title" if found.title.is_none() => title = Some(String::new()),
            "main" => main = true,
            "article" => article = true,
            "base" if found.base.is_none() => {
                found.base = attrs.get("href").map(|h| h.into_owned())
            }
            _ => {}
        },
        Token::End { name } if local_name(name).eq_ignore_ascii_case("title") => {
            if let Some(text) = title.take() {
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                found.title = (!text.is_empty()).then_some(text);
            }
        }
        Token::Text(text) => {
            if let Some(title) = title.as_mut()
                && title.len() < TITLE
            {
                title.push_str(&decode(&text));
            }
        }
        _ => {}
    };
    let mut tokenizer = Tokenizer::html();
    let mut chunk = [0u8; 1024];
    let mut at = 0u64;
    loop {
        let n = storage.read_at(path, at, &mut chunk)?;
        if n == 0 {
            break;
        }
        tokenizer.feed(&chunk[..n], &mut handle);
        at += n as u64;
    }
    tokenizer.finish(&mut handle);
    found.focus = if main {
        Some("main")
    } else if article {
        Some("article")
    } else {
        None
    };
    found.language = found.language.filter(|l| !l.is_empty());
    Ok(found)
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl HtmlDocument {
    /// Opens an HTML file; without `styles` its CSS is ignored.
    pub fn open(storage: &dyn Storage, path: &str, styles: bool) -> Result<Self, DocumentError> {
        let size = storage.size(path)?;
        let found = scan(storage, path)?;
        let mut cache = format!(
            "{}/html-{:016x}",
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
            xpointers: format!("{cache}/0.xpt"),
            end: format!("{cache}/0.end"),
        };
        // Converted again when the file or the cache format changed.
        let mut head = [0u8; 4096];
        let n = storage.read_at(path, 0, &mut head)?;
        let stamp = format!(
            "{}.{HTML_VERSION} {size} {:016x}\n",
            crate::package::CACHE_VERSION,
            fnv(&head[..n])
        );
        let stamp_path = format!("{cache}/source");
        if quireink_sdk::read_file(storage, &stamp_path, 256).ok()
            != Some(stamp.clone().into_bytes())
        {
            let _ = storage.remove(&paths.end);
            storage.write(&stamp_path, stamp.as_bytes())?;
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        let name = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        let metadata = Metadata {
            title: found.title.or_else(|| Some(name.to_string())),
            authors: Vec::new(),
            language: found.language,
            ..Metadata::default()
        };
        Ok(Self {
            path: path.to_string(),
            paths,
            metadata,
            focus: found.focus,
            base: found.base,
            styles,
            size: u32::try_from(size).unwrap_or(u32::MAX - 1),
            length: Cell::new(UNKNOWN),
        })
    }

    /// Converts the page if needed; returns its text length and number of
    /// blocks.
    fn converted(&self, storage: &dyn Storage) -> Result<(u32, u32), DocumentError> {
        let length = match cached::u32_at(storage, &self.paths.end, 0) {
            Ok(length) => length,
            Err(_) => {
                let source = Source::Html {
                    path: &self.path,
                    focus: self.focus,
                    base: self.base.as_deref(),
                };
                // Web pages' pictures are not fetched: no colour to ask for.
                convert::convert(
                    storage,
                    &source,
                    &self.paths,
                    Default::default(),
                    self.styles,
                    false,
                )?
            }
        };
        self.length.set(length);
        let blocks = (storage.size(&self.paths.index)? / u64::from(INDEX)) as u32;
        Ok((length, blocks))
    }
}

impl Document for HtmlDocument {
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
        cached::block_at(storage, &self.paths, length, blocks, at.offset, false)
    }

    fn links(&self, storage: &dyn Storage, section: u32, from: u32, to: u32) -> Vec<Link> {
        if section != 0 || self.converted(storage).is_err() {
            return Vec::new();
        }
        cached::links(storage, &self.paths, from, to)
    }

    fn link_target(&self, storage: &dyn Storage, href: &str) -> Option<Locator> {
        let id = href.strip_prefix('#')?;
        self.converted(storage).ok()?;
        cached::anchor(storage, &self.paths, id).map(|offset| Locator::new(0, offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;
    use quireink_sdk::document::BlockKind;

    const PAGE: &str = r##"<!DOCTYPE html>
<html lang="de"><head><meta charset="utf-8"><title>Ein   Artikel</title>
<base href="https://example.org/news/item.html">
<script>if (a<b) { document.write("</div>"); }</script>
<style>.hidden { display: none }</style></head>
<body><header><nav><a href="/">Start</a> <a href="/news">News</a></nav></header>
<aside>Werbung</aside>
<main><article><header><h1>Überschrift</h1></header>
<p>Erster Absatz mit <a href="../more.html">Link</a> und <a href="#n1">Fußnote</a>
<p>Zweiter Absatz<br>neue Zeile
<ul><li>Eins<li>Zwei</ul>
<p class="hidden">Versteckt</p>
<form><input name="q"><button>Suchen</button></form>
<p id="n1">Die Fußnote.</p></article></main>
<footer>Impressum</footer></body></html>"##;

    #[test]
    fn web_pages_show_their_reading_matter() {
        let storage = MemoryStorage::new().with("web/page.html", PAGE.as_bytes().to_vec());
        let doc = HtmlDocument::open(&storage, "web/page.html", true).unwrap();
        assert_eq!(doc.metadata().title.as_deref(), Some("Ein Artikel"));
        assert_eq!(doc.metadata().language.as_deref(), Some("de"));
        let mut texts = Vec::new();
        let mut at = Locator::START;
        while let Some(block) = doc.block_at(&storage, at).unwrap() {
            texts.push((block.kind, block.text.clone()));
            at = Locator::new(0, block.end);
        }
        let only: Vec<&str> = texts.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            only,
            [
                "Überschrift",
                "Erster Absatz mit Link und Fußnote",
                "Zweiter Absatz\nneue Zeile",
                "• Eins",
                "• Zwei",
                "Die Fußnote."
            ]
        );
        assert_eq!(texts[0].0, BlockKind::Heading(1));
        let links = doc.links(&storage, 0, 0, u32::MAX);
        let hrefs: Vec<&str> = links.iter().map(|l| l.href.as_str()).collect();
        assert_eq!(hrefs, ["https://example.org/more.html", "#n1"]);
        assert_eq!(doc.link_target(&storage, &links[0].href), None);
        let note = doc.link_target(&storage, "#n1").unwrap();
        let block = doc.block_at(&storage, note).unwrap().unwrap();
        assert_eq!(block.text, "Die Fußnote.");
        // Opened again: from the cache.
        let doc = HtmlDocument::open(&storage, "web/page.html", true).unwrap();
        assert_eq!(doc.links(&storage, 0, 0, u32::MAX).len(), 2);
    }

    #[test]
    fn small_caps_reach_the_runs() {
        let page = "<html><head><style>.sc { font-variant: small-caps }</style></head><body>\
            <p>At <span class=\"sc\">noon</span> and <b style=\"font-variant-caps: \
            all-small-caps\">later</b>.</p></body></html>";
        let storage = MemoryStorage::new().with("p.html", page.as_bytes().to_vec());
        let doc = HtmlDocument::open(&storage, "p.html", true).unwrap();
        let block = doc.block_at(&storage, Locator::START).unwrap().unwrap();
        assert_eq!(block.text, "At noon and later.");
        let caps: Vec<(&str, bool)> = block
            .runs
            .iter()
            .filter(|r| r.style.small_caps())
            .map(|r| {
                (
                    &block.text[r.start as usize..r.end as usize],
                    r.style.bold(),
                )
            })
            .collect();
        assert_eq!(caps, [("noon", false), ("later", true)]);
    }

    #[test]
    fn pages_without_marked_reading_matter_lose_their_furniture() {
        let page = "<html><body><header>Seite</header><nav>Menü</nav>\
            <div><p>Text</p></div><footer>Fuß</footer></body></html>";
        let storage = MemoryStorage::new().with("p.htm", page.as_bytes().to_vec());
        let doc = HtmlDocument::open(&storage, "p.htm", true).unwrap();
        assert_eq!(doc.metadata().title.as_deref(), Some("p"));
        let block = doc.block_at(&storage, Locator::START).unwrap().unwrap();
        assert_eq!(block.text, "Text");
        assert!(
            doc.block_at(&storage, Locator::new(0, block.end))
                .unwrap()
                .is_none()
        );
    }
}

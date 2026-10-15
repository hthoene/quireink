//! Comic book archives (`.cbz`): a ZIP of pictures, one page each, in the
//! order of their names (numbers counted as numbers, so `2.jpg` comes
//! before `10.jpg`). A `ComicInfo.xml` gives title, series, number,
//! writers and summary.
//!
//! Each picture is a section of one image block; it is decoded when its
//! page is first shown and kept in the cache, so only one page is ever
//! decoded at a time, also on small devices.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

use quireink_sdk::document::{
    Align, Block, BlockKind, Document, DocumentError, ImageRef, Locator, Metadata, OffsetMap,
};
use quireink_sdk::image::GrayFile;
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::zip::ZipArchive;
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;
use crate::convert;
use crate::package::error;

/// Pages kept at most (names held in memory).
const MAX_PAGES: usize = 5000;
/// Largest `ComicInfo.xml` read.
const MAX_INFO: usize = 64 * 1024;

/// An open comic book.
#[derive(Debug)]
pub struct CbzDocument {
    archive: ZipArchive,
    /// The pictures' names in the archive, in reading order.
    pages: Vec<String>,
    cache: String,
    metadata: Metadata,
}

/// Whether `name` is a picture QuireInk shows.
fn is_picture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !lower.starts_with("__macosx/")
        && !lower.rsplit('/').next().unwrap_or("").starts_with('.')
        && [".jpg", ".jpeg", ".png"].iter().any(|e| lower.ends_with(e))
}

/// Names in reading order: numbers within them by value.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let digits = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).count();
                let (n, m) = (digits(a), digits(b));
                let trim = |s: &[u8]| -> usize { s.iter().take_while(|&&c| c == b'0').count() };
                let (x0, y0) = (trim(&a[..n]).min(n - 1), trim(&b[..m]).min(m - 1));
                let (x, y) = (&a[x0..n], &b[y0..m]);
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if order != Ordering::Equal {
                    return order;
                }
                a = &a[n..];
                b = &b[m..];
            }
            (Some(x), Some(y)) => {
                let order = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl CbzDocument {
    pub fn open(storage: &dyn Storage, path: &str) -> Result<Self, DocumentError> {
        let archive = ZipArchive::open(storage, path).map_err(error)?;
        let mut pages: Vec<String> = Vec::new();
        let mut info = None;
        for entry in archive.entries(storage).filter_map(Result::ok) {
            if entry.name.eq_ignore_ascii_case("ComicInfo.xml") {
                info = Some(entry);
            } else if is_picture(&entry.name) && pages.len() < MAX_PAGES {
                pages.push(entry.name);
            }
        }
        if pages.is_empty() {
            return Err(DocumentError::Malformed(alloc::borrow::Cow::Borrowed(
                "no pictures",
            )));
        }
        pages.sort_by(|a, b| natural(a, b));
        let cache = format!(
            "{}/cbz-{:016x}",
            Env::plugin_dir(PLUGIN.as_str()),
            fnv(path.as_bytes())
        );
        // Pictures decoded from another file of the same name go.
        let stamp = format!("{} {}\n", storage.size(path)?, pages.len());
        let stamp_path = format!("{cache}/source");
        if quireink_sdk::read_file(storage, &stamp_path, 64).ok()
            != Some(stamp.clone().into_bytes())
        {
            let _ = storage.remove_dir_all(&cache);
            let _ = storage.create_dir(&cache);
            storage.write(&stamp_path, stamp.as_bytes())?;
        }
        let mut metadata = info
            .and_then(|entry| comic_info(storage, &archive, &entry))
            .unwrap_or_default();
        if metadata.title.is_none() {
            let name = path.rsplit('/').next().unwrap_or(path);
            metadata.title = Some(name.rsplit_once('.').map_or(name, |(s, _)| s).to_string());
        }
        Ok(Self {
            archive,
            pages,
            cache,
            metadata,
        })
    }

    /// Page `index` decoded at most `side` pixels on its longer side into
    /// `cache`, if it can be.
    fn picture(
        &self,
        storage: &dyn Storage,
        index: usize,
        cache: &str,
        side: u32,
    ) -> Option<ImageRef> {
        if GrayFile::open(storage, cache).is_none() {
            let name = self.pages.get(index)?;
            convert::decode_image(storage, &self.archive, name, cache, &self.cache, side).ok()?;
        }
        GrayFile::open(storage, cache).map(|image| ImageRef {
            path: image.path,
            width: image.width,
            height: image.height,
        })
    }
}

/// Title, series and number, writers, summary, language and genres from
/// `ComicInfo.xml`.
fn comic_info(
    storage: &dyn Storage,
    archive: &ZipArchive,
    entry: &quireink_sdk::zip::Entry,
) -> Option<Metadata> {
    let mut data = Vec::new();
    archive
        .stream(storage, entry, &mut |chunk| {
            data.extend_from_slice(chunk);
            data.len() < MAX_INFO
        })
        .ok()?;
    let mut metadata = Metadata::default();
    let mut field = String::new();
    let mut text = String::new();
    let list = |text: &str| -> Vec<String> {
        text.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    };
    Tokenizer::parse(&data, &mut |token| match token {
        Token::Start { name, .. } => {
            field = local_name(name).to_string();
            text.clear();
        }
        Token::Text(t) => text.push_str(&decode(&t)),
        Token::End { name } => {
            let value = text.trim().to_string();
            if local_name(name) == field && !value.is_empty() {
                match field.as_str() {
                    "Title" => metadata.title = Some(value),
                    "Series" => metadata.series = Some(value),
                    "Number" => metadata.series_index = Some(value),
                    "Writer" => metadata.authors = list(&value),
                    "Summary" => metadata.description = Some(value),
                    "LanguageISO" => metadata.language = Some(value),
                    "Genre" | "Tags" => metadata.subjects.extend(list(&value)),
                    _ => {}
                }
            }
            text.clear();
        }
        _ => {}
    });
    // An issue's title alone says little: with its series.
    if let (Some(series), Some(title)) = (&metadata.series, &metadata.title)
        && !title.contains(series.as_str())
    {
        metadata.title = Some(format!("{series}: {title}"));
    } else if metadata.title.is_none()
        && let Some(series) = &metadata.series
    {
        metadata.title = Some(match &metadata.series_index {
            Some(number) => format!("{series} {number}"),
            None => series.clone(),
        });
    }
    Some(metadata)
}

impl Document for CbzDocument {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn sections(&self) -> u32 {
        self.pages.len() as u32
    }

    fn section_len(&self, _: u32) -> u32 {
        1
    }

    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError> {
        let index = at.section as usize;
        if index >= self.pages.len() || at.offset >= 1 {
            return Ok(None);
        }
        let cache = format!("{}/p{index}.gray", self.cache);
        let image = self.picture(storage, index, &cache, convert::IMAGE_SIDE);
        // A picture that cannot be shown is named instead.
        let (kind, text) = match &image {
            Some(_) => (BlockKind::Image, String::new()),
            None => (BlockKind::Paragraph, self.pages[index].clone()),
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

    fn cover(&self, storage: &dyn Storage) -> Result<Option<ImageRef>, DocumentError> {
        let cache = format!("{}/cover.gray", self.cache);
        Ok(self.picture(storage, 0, &cache, convert::COVER_SIDE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;
    use quireink_sdk::zip::testing::ZipBuilder;

    #[test]
    fn pages_in_natural_order() {
        let mut names = ["p10.jpg", "p2.jpg", "P1.jpg", "p02b.jpg"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["P1.jpg", "p2.jpg", "p02b.jpg", "p10.jpg"]);
        assert!(is_picture("a/1.PNG"));
        assert!(!is_picture("__MACOSX/1.png"));
        assert!(!is_picture("ComicInfo.xml"));
    }

    #[test]
    fn opens_pages_and_comic_info() {
        let info = "<?xml version=\"1.0\"?><ComicInfo><Title>The Start</Title>\
            <Series>Tales</Series><Number>1</Number><Writer>Ann, Bob</Writer>\
            <Summary>It begins.</Summary></ComicInfo>";
        let zip = ZipBuilder::new()
            .stored("10.jpg", b"not a picture".to_vec())
            .stored("2.jpg", b"not one either".to_vec())
            .deflated("ComicInfo.xml", info.as_bytes())
            .build();
        let storage = MemoryStorage::new().with("c/t.cbz", zip);
        let doc = CbzDocument::open(&storage, "c/t.cbz").unwrap();
        let m = doc.metadata();
        assert_eq!(m.title.as_deref(), Some("Tales: The Start"));
        assert_eq!(m.authors, ["Ann", "Bob"]);
        assert_eq!(m.series_index.as_deref(), Some("1"));
        assert_eq!(m.description.as_deref(), Some("It begins."));
        assert_eq!(doc.sections(), 2);
        // Pictures that cannot be decoded are named.
        let block = doc.block_at(&storage, Locator::new(0, 0)).unwrap().unwrap();
        assert_eq!(block.text, "2.jpg");
        assert!(
            doc.block_at(&storage, Locator::new(0, 1))
                .unwrap()
                .is_none()
        );
        assert!(
            doc.block_at(&storage, Locator::new(2, 0))
                .unwrap()
                .is_none()
        );
    }
}

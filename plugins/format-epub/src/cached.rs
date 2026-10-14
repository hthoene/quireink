//! Reading converted chapters from the cache (see [`crate::convert`]):
//! blocks, anchors and links, for EPUB chapters and HTML pages alike.

use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::document::{
    Align, Block, BlockKind, DocumentError, ImageRef, Link, OffsetMap, Run, TextStyle,
};
use quireink_sdk::image::GrayFile;
use quireink_sdk::{Storage, StorageError};

use crate::convert::{self, CachePaths, HEADER, INDEX};

pub(crate) fn u32_at(storage: &dyn Storage, path: &str, offset: u64) -> Result<u32, StorageError> {
    let mut buf = [0u8; 4];
    if storage.read_at(path, offset, &mut buf)? != 4 {
        return Err(StorageError::Io);
    }
    Ok(u32::from_le_bytes(buf))
}

/// Reads block `i` of a chapter.
pub(crate) fn read_block(
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
    let at =
        start
            .checked_sub(HEADER)
            .ok_or(DocumentError::Malformed(alloc::borrow::Cow::Borrowed(
                "cache index",
            )))?;
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
        indent: Some(i16::from_le_bytes([entry[4], entry[5]])).filter(|&i| i != convert::NO_INDENT),
        space: Some(u16::from_le_bytes([entry[6], entry[7]])).filter(|&s| s != convert::NO_SPACE),
        size: Some(u16::from_le_bytes([entry[8], entry[9]])).filter(|&s| s != 0),
        image,
    })
}

/// The block of a converted chapter containing `offset`, or the first
/// after it; `None` past its end.
pub(crate) fn block_at(
    storage: &dyn Storage,
    paths: &CachePaths,
    length: u32,
    blocks: u32,
    offset: u32,
) -> Result<Option<Block>, DocumentError> {
    if blocks == 0 || offset >= length {
        return Ok(None);
    }
    // The last block whose text starts at or before the offset.
    let (mut low, mut high) = (0u32, blocks);
    while low < high {
        let mid = low + (high - low) / 2;
        if u32_at(storage, &paths.index, u64::from(mid * INDEX))? <= offset {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    read_block(storage, paths, length, blocks, low.saturating_sub(1)).map(Some)
}

/// Calls `visit` with each line of a cache file until it returns
/// `false`, reading in small pieces.
pub(crate) fn lines(storage: &dyn Storage, path: &str, mut visit: impl FnMut(&str) -> bool) {
    let mut chunk = [0u8; 256];
    let mut line: Vec<u8> = Vec::new();
    let mut offset = 0u64;
    loop {
        let Ok(n) = storage.read_at(path, offset, &mut chunk) else {
            return;
        };
        offset += n as u64;
        for &byte in &chunk[..n] {
            if byte != b'\n' {
                // Longer lines are cut; nothing sensible is that long.
                if line.len() < 2048 {
                    line.push(byte);
                }
                continue;
            }
            if !visit(core::str::from_utf8(&line).unwrap_or_default()) {
                return;
            }
            line.clear();
        }
        if n == 0 {
            return;
        }
    }
}

/// The offset of anchor `id` in a converted chapter.
pub(crate) fn anchor(storage: &dyn Storage, paths: &CachePaths, id: &str) -> Option<u32> {
    let mut found = None;
    lines(storage, &paths.anchors, |line| {
        if let Some((name, offset)) = line.split_once('\t')
            && name == id
        {
            found = offset.parse().ok();
            return false;
        }
        true
    });
    found
}

/// The links of a converted chapter that overlap `from..to`, in order.
pub(crate) fn links(storage: &dyn Storage, paths: &CachePaths, from: u32, to: u32) -> Vec<Link> {
    let mut links = Vec::new();
    lines(storage, &paths.links, |line| {
        let mut parts = line.splitn(3, '\t');
        let (Some(start), Some(end), Some(href)) = (parts.next(), parts.next(), parts.next())
        else {
            return true;
        };
        let (Ok(start), Ok(end)) = (start.parse::<u32>(), end.parse::<u32>()) else {
            return true;
        };
        if start >= to {
            return false;
        }
        if end > from {
            links.push(Link {
                start,
                end,
                href: String::from(href),
            });
        }
        true
    });
    links
}

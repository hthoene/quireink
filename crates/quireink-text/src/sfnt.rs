//! Reading a font file's family name and style from its header, without
//! loading the font: only the table directory and the small `name`,
//! `OS/2` and `head` tables are read, so a folder of large fonts can be
//! listed in little memory and time.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// What a font file says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontInfo {
    /// The family, e.g. `Literata`; shared by the regular, bold and italic
    /// files of a family.
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// Reads `buf.len()` bytes at an offset; returns how many were read.
pub type ReadAt<'a> = &'a mut dyn FnMut(u64, &mut [u8]) -> usize;

/// Largest `name` table read.
const MAX_NAME_TABLE: u32 = 64 * 1024;

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

fn read(read_at: ReadAt<'_>, offset: u64, len: u32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len as usize];
    (read_at(offset, &mut buf) == buf.len()).then_some(buf)
}

/// The family and style of an OpenType/TrueType font (the first font of a
/// collection); `None` if it is none.
pub fn read_info(read_at: ReadAt<'_>) -> Option<FontInfo> {
    let header = read(read_at, 0, 12)?;
    let mut base = 0u64;
    let header = match &header[..4] {
        // A collection: use its first font.
        b"ttcf" => {
            let first = read(read_at, 12, 4)?;
            base = u64::from(u32_at(&first, 0)?);
            read(read_at, base, 12)?
        }
        [0, 1, 0, 0] | b"OTTO" | b"true" => header,
        _ => return None,
    };
    let tables = u16_at(&header, 4)?;
    let directory = read(read_at, base + 12, u32::from(tables) * 16)?;
    let table = |tag: &[u8; 4]| -> Option<(u64, u32)> {
        (0..usize::from(tables)).find_map(|i| {
            let record = &directory[i * 16..i * 16 + 16];
            (&record[..4] == tag).then(|| {
                (
                    u64::from(u32_at(record, 8).unwrap_or(0)),
                    u32_at(record, 12).unwrap_or(0),
                )
            })
        })
    };

    let (offset, len) = table(b"name")?;
    let name = read(read_at, offset, len.min(MAX_NAME_TABLE))?;
    let family = family_name(&name)?;

    // Style: OS/2 (weight and selection flags), else the head table.
    let (mut bold, mut italic) = (false, false);
    if let Some((offset, len)) = table(b"OS/2").filter(|(_, len)| *len >= 64) {
        let os2 = read(read_at, offset, len.min(96))?;
        let weight = u16_at(&os2, 4)?;
        let selection = u16_at(&os2, 62)?;
        italic = selection & 1 != 0;
        bold = selection & (1 << 5) != 0 || weight >= 600;
    } else if let Some((offset, _)) = table(b"head") {
        let head = read(read_at, offset, 54)?;
        let style = u16_at(&head, 44)?;
        bold = style & 1 != 0;
        italic = style & 2 != 0;
    }
    Some(FontInfo {
        family,
        bold,
        italic,
    })
}

/// The typographic family name (name id 16), else the family name (1);
/// English names first.
fn family_name(table: &[u8]) -> Option<String> {
    let count = usize::from(u16_at(table, 2)?);
    let strings = usize::from(u16_at(table, 4)?);
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let r = 6 + i * 12;
        let (platform, language, id) = (
            u16_at(table, r)?,
            u16_at(table, r + 4)?,
            u16_at(table, r + 6)?,
        );
        let (len, offset) = (
            usize::from(u16_at(table, r + 8)?),
            usize::from(u16_at(table, r + 10)?),
        );
        if id != 1 && id != 16 {
            continue;
        }
        let Some(bytes) = table.get(strings + offset..strings + offset + len) else {
            continue;
        };
        let text = match platform {
            // Windows and Unicode: UTF-16BE.
            0 | 3 => String::from_utf16_lossy(
                &bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect::<Vec<_>>(),
            ),
            // Macintosh: Roman, here only ASCII.
            1 => bytes.iter().map(|&b| char::from(b.min(0x7f))).collect(),
            _ => continue,
        };
        let english = language == 0x409 || (platform == 1 && language == 0);
        let rank = u8::from(id == 16) * 2 + u8::from(english);
        if !text.trim().is_empty() && best.as_ref().is_none_or(|(r, _)| rank > *r) {
            best = Some((rank, String::from(text.trim())));
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(data: &[u8]) -> Option<FontInfo> {
        read_info(&mut |offset, buf: &mut [u8]| {
            let start = (offset as usize).min(data.len());
            let n = buf.len().min(data.len() - start);
            buf[..n].copy_from_slice(&data[start..start + n]);
            n
        })
    }

    #[test]
    fn reads_family_and_style() {
        let regular = info(include_bytes!(
            "../../../plugins/fonts-default/fonts/Literata.ttf"
        ));
        assert_eq!(
            regular,
            Some(FontInfo {
                family: String::from("Literata"),
                bold: false,
                italic: false
            })
        );
        let italic = info(include_bytes!(
            "../../../plugins/fonts-default/fonts/Literata-Italic.ttf"
        ))
        .unwrap();
        assert_eq!(italic.family, "Literata");
        assert!(italic.italic && !italic.bold);
        assert_eq!(info(b"not a font at all"), None);
    }
}

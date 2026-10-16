//! WOFF (version 1): an OpenType or TrueType font whose tables are
//! compressed one by one with zlib. Turned back into the font it was, so
//! it is read as any embedded font. WOFF 2 (Brotli) is not read.

use alloc::vec::Vec;

use miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;

/// The start of a WOFF file.
pub(crate) const SIGNATURE: &[u8; 4] = b"wOFF";

const HEADER: usize = 44;
const ENTRY: usize = 20;

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The font `woff` holds, at most `max` bytes; `None` if it is not a
/// well-formed WOFF.
pub(crate) fn to_font(woff: &[u8], max: usize) -> Option<Vec<u8>> {
    if woff.get(..4)? != SIGNATURE {
        return None;
    }
    let flavor = u32_at(woff, 4)?;
    let tables = usize::from(u16_at(woff, 12)?);
    let total = u32_at(woff, 16)? as usize;
    if tables == 0 || total > max || woff.len() < HEADER + tables * ENTRY {
        return None;
    }
    // The font's own directory: version, number of tables and the binary
    // search fields, then a record per table.
    let mut font = Vec::new();
    font.try_reserve_exact(total.max(12 + tables * 16)).ok()?;
    let power = 1usize << (usize::BITS - 1 - tables.leading_zeros());
    let search = power * 16;
    font.extend_from_slice(&flavor.to_be_bytes());
    font.extend_from_slice(&(tables as u16).to_be_bytes());
    font.extend_from_slice(&(search as u16).to_be_bytes());
    font.extend_from_slice(&(power.trailing_zeros() as u16).to_be_bytes());
    font.extend_from_slice(&((tables * 16 - search) as u16).to_be_bytes());
    let records = font.len();
    font.resize(records + tables * 16, 0);
    for n in 0..tables {
        let entry = HEADER + n * ENTRY;
        let tag = woff.get(entry..entry + 4)?;
        let offset = u32_at(woff, entry + 4)? as usize;
        let compressed = u32_at(woff, entry + 8)? as usize;
        let length = u32_at(woff, entry + 12)? as usize;
        let checksum = u32_at(woff, entry + 16)?;
        let stored = woff.get(offset..offset.checked_add(compressed)?)?;
        let at = font.len();
        if at + length > max.max(total) {
            return None;
        }
        if compressed < length {
            let table = decompress_to_vec_zlib_with_limit(stored, length).ok()?;
            if table.len() != length {
                return None;
            }
            font.extend_from_slice(&table);
        } else if compressed == length {
            font.extend_from_slice(stored);
        } else {
            return None;
        }
        // Tables start on four bytes.
        while font.len() % 4 != 0 {
            font.push(0);
        }
        let record = records + n * 16;
        font[record..record + 4].copy_from_slice(tag);
        font[record + 4..record + 8].copy_from_slice(&checksum.to_be_bytes());
        font[record + 8..record + 12].copy_from_slice(&(at as u32).to_be_bytes());
        font[record + 12..record + 16].copy_from_slice(&(length as u32).to_be_bytes());
    }
    Some(font)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTF: &[u8] = include_bytes!("../tests/data/atkinson-subset.ttf");
    const WOFF: &[u8] = include_bytes!("../tests/data/atkinson-subset.woff");

    /// The tables of a font: tag and contents, by tag.
    fn tables(font: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let count = usize::from(u16_at(font, 4).unwrap());
        let mut out: Vec<([u8; 4], &[u8])> = (0..count)
            .map(|n| {
                let record = 12 + n * 16;
                let tag: [u8; 4] = font[record..record + 4].try_into().unwrap();
                let offset = u32_at(font, record + 8).unwrap() as usize;
                let length = u32_at(font, record + 12).unwrap() as usize;
                (tag, &font[offset..offset + length])
            })
            .collect();
        out.sort_by_key(|(tag, _)| *tag);
        out
    }

    #[test]
    fn woff_turns_back_into_its_font() {
        let font = to_font(WOFF, 1 << 20).expect("decodes");
        assert_eq!(&font[..4], &TTF[..4]);
        assert_eq!(tables(&font), tables(TTF));
    }

    #[test]
    fn damaged_or_too_large_fonts_are_refused() {
        assert_eq!(to_font(TTF, 1 << 20), None);
        assert_eq!(to_font(WOFF, 100), None);
        for cut in [4, 44, 100, WOFF.len() / 2, WOFF.len() - 3] {
            assert_eq!(to_font(&WOFF[..cut], 1 << 20), None, "cut at {cut}");
        }
    }
}

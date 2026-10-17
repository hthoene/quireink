//! Pre-rendered bitmap font packs (`.qifp`).
//!
//! Packs hold glyph bitmaps for a set of pixel sizes ("strikes"). They are
//! read in place (on devices that map flash into the address space they use
//! no RAM at all) and need no floating-point work at runtime.
//!
//! # Format (version 1, little endian)
//!
//! ```text
//! header   12 bytes  "QIFP", version u8, 3 reserved, strike count u16, reserved u16
//! strikes  32 bytes  size (1/4 px) u16, reserved u16, ascent, descent,
//!   each             line height (all i32, 1/64 px), glyph count u32,
//!                    glyph table offset u32, bitmap offset u32, reserved u32
//! glyphs   20 bytes  code point u32, advance i32 (1/64 px), left i16, top i16,
//!   each, sorted     width u16, height u16, data offset u32 (from bitmap offset)
//! bitmaps            4-bit coverage, two pixels per byte, rows padded
//! ```

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

use crate::font::{FontError, LineMetrics};
use crate::glyph::{Glyph, GlyphRef};

const MAGIC: &[u8; 4] = b"QIFP";
const VERSION: u8 = 1;
const HEADER: usize = 12;
const STRIKE: usize = 32;
const RECORD: usize = 20;

/// A bitmap font pack.
#[derive(Clone)]
pub struct BitmapFont {
    data: Cow<'static, [u8]>,
    strikes: u16,
}

impl fmt::Debug for BitmapFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BitmapFont")
            .field("bytes", &self.data.len())
            .field("strikes", &self.strikes)
            .finish()
    }
}

/// A view of one strike's header.
#[derive(Debug, Clone, Copy)]
struct Strike {
    size: f32,
    metrics: LineMetrics,
    glyphs: usize,
    glyph_table: usize,
    bitmaps: usize,
}

impl BitmapFont {
    pub fn is_pack(data: &[u8]) -> bool {
        data.starts_with(MAGIC)
    }

    /// Validates the pack's structure.
    pub fn new(data: impl Into<Cow<'static, [u8]>>) -> Result<BitmapFont, FontError> {
        let data = data.into();
        if !Self::is_pack(&data) || data.len() < HEADER {
            return Err(FontError::Invalid);
        }
        if data[4] != VERSION {
            return Err(FontError::UnsupportedVersion);
        }
        let strikes = u16_at(&data, 8).ok_or(FontError::Invalid)?;
        let font = BitmapFont { data, strikes };
        if strikes == 0 {
            return Err(FontError::Invalid);
        }
        for i in 0..usize::from(strikes) {
            let strike = font.strike(i).ok_or(FontError::Invalid)?;
            let end = strike.glyph_table + strike.glyphs * RECORD;
            if end > font.data.len() || strike.bitmaps > font.data.len() {
                return Err(FontError::Invalid);
            }
        }
        Ok(font)
    }

    /// Available sizes in pixels.
    pub fn sizes(&self) -> impl Iterator<Item = f32> + '_ {
        (0..usize::from(self.strikes)).filter_map(|i| self.strike(i).map(|s| s.size))
    }

    fn strike(&self, index: usize) -> Option<Strike> {
        let base = HEADER + index * STRIKE;
        let d = &self.data;
        Some(Strike {
            size: f32::from(u16_at(d, base)?) / 4.0,
            metrics: LineMetrics {
                ascent: q6(i32_at(d, base + 4)?),
                descent: q6(i32_at(d, base + 8)?),
                line_height: q6(i32_at(d, base + 12)?),
            },
            glyphs: u32_at(d, base + 16)? as usize,
            glyph_table: u32_at(d, base + 20)? as usize,
            bitmaps: u32_at(d, base + 24)? as usize,
        })
    }

    /// The index of the strike closest to `size`, in whole numbers as the
    /// pack keeps sizes (quarter pixels): asked for every character drawn
    /// or measured, on chips that reckon in floats slowly.
    fn nearest_index(&self, size: f32) -> Option<usize> {
        let wanted = (size * 4.0) as i32;
        (0..usize::from(self.strikes))
            .filter_map(|i| Some((i, i32::from(u16_at(&self.data, HEADER + i * STRIKE)?))))
            .min_by_key(|&(_, quarters)| (quarters - wanted).abs())
            .map(|(i, _)| i)
    }

    /// The strike closest to `size`.
    fn nearest(&self, size: f32) -> Option<Strike> {
        self.strike(self.nearest_index(size)?)
    }

    pub fn line_metrics(&self, size: f32) -> LineMetrics {
        self.nearest(size)
            .map_or(LineMetrics::fallback(size), |s| s.metrics)
    }

    /// Where a strike's glyphs are: their number, their table and their
    /// bitmaps (what finding a glyph needs, without the metrics).
    fn tables(&self, index: usize) -> Option<Tables> {
        let base = HEADER + index * STRIKE;
        let d = &self.data;
        Some(Tables {
            glyphs: u32_at(d, base + 16)? as usize,
            glyph_table: u32_at(d, base + 20)? as usize,
            bitmaps: u32_at(d, base + 24)? as usize,
        })
    }

    fn record(&self, strike: &Tables, ch: char) -> Option<usize> {
        let (mut low, mut high) = (0usize, strike.glyphs);
        let target = ch as u32;
        while low < high {
            let mid = (low + high) / 2;
            let offset = strike.glyph_table + mid * RECORD;
            let code = u32_at(&self.data, offset)?;
            match code.cmp(&target) {
                core::cmp::Ordering::Less => low = mid + 1,
                core::cmp::Ordering::Greater => high = mid,
                core::cmp::Ordering::Equal => return Some(offset),
            }
        }
        None
    }

    pub fn has_char(&self, ch: char) -> bool {
        self.nearest_index(0.0)
            .and_then(|i| self.tables(i))
            .is_some_and(|strike| self.record(&strike, ch).is_some())
    }

    /// The glyph for `ch` at the strike nearest to `size`. Missing
    /// characters fall back to U+FFFD, then `?`.
    pub fn glyph(&self, ch: char, size: f32) -> GlyphRef<'_> {
        let empty = GlyphRef {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            advance: 0.0,
            data: &[],
        };
        let Some(strike) = self.nearest_index(size).and_then(|i| self.tables(i)) else {
            return empty;
        };
        let Some(offset) = [ch, '\u{fffd}', '?']
            .into_iter()
            .find_map(|c| self.record(&strike, c))
        else {
            return empty;
        };
        let d = &self.data;
        let read = || -> Option<GlyphRef<'_>> {
            let width = u16_at(d, offset + 12)?;
            let height = u16_at(d, offset + 14)?;
            let start = strike.bitmaps + u32_at(d, offset + 16)? as usize;
            let len = usize::from(width).div_ceil(2) * usize::from(height);
            Some(GlyphRef {
                advance: q6(i32_at(d, offset + 4)?),
                left: i16_at(d, offset + 8)?,
                top: i16_at(d, offset + 10)?,
                width,
                height,
                data: d.get(start..start + len)?,
            })
        };
        read().unwrap_or(empty)
    }

    pub fn advance(&self, ch: char, size: f32) -> f32 {
        self.glyph(ch, size).advance
    }
}

/// Where a strike's glyphs are (see [`BitmapFont::tables`]).
struct Tables {
    glyphs: usize,
    glyph_table: usize,
    bitmaps: usize,
}

/// Builds a pack from rasterized glyphs (used by `quireink-fontgen`).
#[derive(Debug, Default)]
pub struct PackBuilder {
    strikes: Vec<StrikeData>,
}

#[derive(Debug)]
struct StrikeData {
    size: f32,
    metrics: LineMetrics,
    glyphs: Vec<(char, Glyph)>,
}

impl PackBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a strike; glyphs may come in any order.
    pub fn add_strike(&mut self, size: f32, metrics: LineMetrics, mut glyphs: Vec<(char, Glyph)>) {
        glyphs.sort_by_key(|(ch, _)| *ch);
        glyphs.dedup_by_key(|(ch, _)| *ch);
        self.strikes.push(StrikeData {
            size,
            metrics,
            glyphs,
        });
    }

    pub fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&[VERSION, 0, 0, 0]);
        out.extend_from_slice(&(self.strikes.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);

        let strike_table = out.len();
        out.resize(strike_table + self.strikes.len() * STRIKE, 0);
        for (index, strike) in self.strikes.iter().enumerate() {
            let (size, metrics, glyphs) = (strike.size, strike.metrics, &strike.glyphs);
            let glyph_table = out.len();
            out.resize(glyph_table + glyphs.len() * RECORD, 0);
            let bitmaps = out.len();
            for (i, (ch, glyph)) in glyphs.iter().enumerate() {
                let data_offset = (out.len() - bitmaps) as u32;
                out.extend_from_slice(&glyph.data);
                let mut record = Vec::with_capacity(RECORD);
                record.extend_from_slice(&(*ch as u32).to_le_bytes());
                record.extend_from_slice(&to_q6(glyph.advance).to_le_bytes());
                record.extend_from_slice(&glyph.left.to_le_bytes());
                record.extend_from_slice(&glyph.top.to_le_bytes());
                record.extend_from_slice(&glyph.width.to_le_bytes());
                record.extend_from_slice(&glyph.height.to_le_bytes());
                record.extend_from_slice(&data_offset.to_le_bytes());
                let at = glyph_table + i * RECORD;
                out[at..at + RECORD].copy_from_slice(&record);
            }
            let mut header = Vec::with_capacity(STRIKE);
            header.extend_from_slice(&((size * 4.0 + 0.5) as u16).to_le_bytes());
            header.extend_from_slice(&[0, 0]);
            header.extend_from_slice(&to_q6(metrics.ascent).to_le_bytes());
            header.extend_from_slice(&to_q6(metrics.descent).to_le_bytes());
            header.extend_from_slice(&to_q6(metrics.line_height).to_le_bytes());
            header.extend_from_slice(&(glyphs.len() as u32).to_le_bytes());
            header.extend_from_slice(&(glyph_table as u32).to_le_bytes());
            header.extend_from_slice(&(bitmaps as u32).to_le_bytes());
            header.extend_from_slice(&[0, 0, 0, 0]);
            let at = strike_table + index * STRIKE;
            out[at..at + STRIKE].copy_from_slice(&header);
        }
        out
    }
}

fn q6(value: i32) -> f32 {
    value as f32 / 64.0
}

fn to_q6(value: f32) -> i32 {
    let scaled = value * 64.0;
    if scaled < 0.0 {
        (scaled - 0.5) as i32
    } else {
        (scaled + 0.5) as i32
    }
}

fn bytes<const N: usize>(data: &[u8], at: usize) -> Option<[u8; N]> {
    data.get(at..at + N)?.try_into().ok()
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    bytes(data, at).map(u16::from_le_bytes)
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    bytes(data, at).map(i16::from_le_bytes)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    bytes(data, at).map(u32::from_le_bytes)
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    bytes(data, at).map(i32::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::pack_coverage;

    fn glyph(width: u16, value: u8, advance: f32) -> Glyph {
        let coverage = alloc::vec![value; usize::from(width) * 2];
        Glyph {
            left: 1,
            top: -2,
            width,
            height: 2,
            advance,
            data: pack_coverage(&coverage, usize::from(width), 2),
        }
    }

    fn pack() -> BitmapFont {
        let metrics = LineMetrics {
            ascent: 8.0,
            descent: 2.0,
            line_height: 12.5,
        };
        let mut builder = PackBuilder::new();
        builder.add_strike(
            10.0,
            metrics,
            alloc::vec![('b', glyph(3, 255, 4.0)), ('a', glyph(2, 136, 3.5))],
        );
        builder.add_strike(20.0, metrics, alloc::vec![('a', glyph(5, 255, 7.0))]);
        BitmapFont::new(builder.build()).unwrap()
    }

    #[test]
    fn round_trips_glyphs_and_metrics() {
        let font = pack();
        assert_eq!(font.sizes().collect::<Vec<_>>(), [10.0, 20.0]);
        let a = font.glyph('a', 11.0);
        assert_eq!((a.width, a.height, a.left, a.top), (2, 2, 1, -2));
        assert_eq!(a.advance, 3.5);
        assert_eq!(a.coverage(1, 1), 136);
        assert_eq!(font.glyph('a', 18.0).width, 5);
        assert_eq!(font.line_metrics(10.0).line_height, 12.5);
        assert!(font.has_char('b'));
    }

    #[test]
    fn missing_characters_fall_back() {
        let font = pack();
        assert!(font.glyph('z', 10.0).is_empty());
        assert_eq!(font.advance('z', 10.0), 0.0);
    }

    #[test]
    fn rejects_corrupt_packs() {
        let mut data = pack().data.into_owned();
        assert!(BitmapFont::new(data[..20].to_vec()).is_err());
        data[4] = 9;
        assert_eq!(
            BitmapFont::new(data).unwrap_err(),
            FontError::UnsupportedVersion
        );
        assert!(BitmapFont::new(b"nope".to_vec()).is_err());
    }
}

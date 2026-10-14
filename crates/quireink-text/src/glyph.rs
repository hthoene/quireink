use alloc::vec::Vec;

/// A borrowed glyph bitmap with 4-bit coverage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphRef<'a> {
    /// Offset of the bitmap's left edge from the pen position.
    pub left: i16,
    /// Offset of the bitmap's top edge from the baseline (negative = above).
    pub top: i16,
    pub width: u16,
    pub height: u16,
    /// Horizontal advance in pixels.
    pub advance: f32,
    /// Two pixels per byte, high nibble first, rows padded to whole bytes.
    pub(crate) data: &'a [u8],
}

impl GlyphRef<'_> {
    /// Coverage of a pixel, 0 (empty) to 255 (full). Out-of-range reads and
    /// truncated data read as empty.
    pub fn coverage(&self, x: u16, y: u16) -> u8 {
        let stride = usize::from(self.width).div_ceil(2);
        let Some(&byte) = self.data.get(usize::from(y) * stride + usize::from(x) / 2) else {
            return 0;
        };
        let nibble = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f };
        nibble * 17
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// An owned glyph bitmap (e.g. rasterized at runtime and cached).
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub left: i16,
    pub top: i16,
    pub width: u16,
    pub height: u16,
    pub advance: f32,
    pub(crate) data: Vec<u8>,
}

impl Glyph {
    /// An empty glyph that only advances the pen (e.g. a space).
    pub fn empty(advance: f32) -> Self {
        Self {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            advance,
            data: Vec::new(),
        }
    }

    pub fn as_ref(&self) -> GlyphRef<'_> {
        GlyphRef {
            left: self.left,
            top: self.top,
            width: self.width,
            height: self.height,
            advance: self.advance,
            data: &self.data,
        }
    }

    /// Approximate heap usage, for cache accounting.
    pub fn byte_size(&self) -> usize {
        self.data.len() + core::mem::size_of::<Glyph>()
    }
}

/// Packs 8-bit coverage into the 4-bit glyph format.
#[cfg_attr(not(feature = "outline"), allow(dead_code))]
pub(crate) fn pack_coverage(coverage: &[u8], width: usize, height: usize) -> Vec<u8> {
    let stride = width.div_ceil(2);
    let mut data = alloc::vec![0u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            let nibble = ((u16::from(coverage[y * width + x]) + 8) / 17) as u8;
            let shift = if x % 2 == 0 { 4 } else { 0 };
            data[y * stride + x / 2] |= nibble << shift;
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_round_trips_through_packing() {
        let coverage = [0, 255, 136, 17, 255];
        let glyph = Glyph {
            left: 0,
            top: 0,
            width: 5,
            height: 1,
            advance: 5.0,
            data: pack_coverage(&coverage, 5, 1),
        };
        let r = glyph.as_ref();
        let read: Vec<u8> = (0..5).map(|x| r.coverage(x, 0)).collect();
        assert_eq!(read, coverage);
        assert_eq!(r.coverage(9, 9), 0);
    }
}

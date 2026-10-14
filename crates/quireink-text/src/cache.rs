use alloc::vec::Vec;

use crate::font::Font;
use crate::glyph::{Glyph, GlyphRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    font: u16,
    /// Size in quarter pixels.
    size: u16,
    ch: char,
}

#[derive(Debug)]
struct Entry {
    key: Key,
    glyph: Glyph,
    last_used: u32,
}

/// A size-bounded least-recently-used cache of glyphs rasterized at runtime.
///
/// Rasterizing is far more expensive than drawing, and a page uses only a
/// few dozen distinct glyphs, so a small cache makes page turns cheap.
/// Bitmap font packs bypass the cache: their glyphs are read in place.
#[derive(Debug)]
pub struct GlyphCache {
    entries: Vec<Entry>,
    bytes: usize,
    capacity: usize,
    clock: u32,
}

impl GlyphCache {
    /// A cache holding at most `capacity` bytes of glyphs.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
            bytes: 0,
            capacity,
            clock: 0,
        }
    }

    /// Bytes currently used.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// The glyph for `ch`. `font_index` identifies `font` within the
    /// caller's font registry.
    #[cfg_attr(not(feature = "outline"), allow(unused_variables))]
    pub fn glyph<'a>(
        &'a mut self,
        font_index: u16,
        font: &'a Font,
        ch: char,
        size: f32,
    ) -> GlyphRef<'a> {
        match font {
            Font::Bitmap(font) => font.glyph(ch, size),
            #[cfg(feature = "outline")]
            Font::Outline(font) => self.cached(font_index, ch, size, |s| font.rasterize(ch, s)),
        }
    }

    #[cfg_attr(not(feature = "outline"), allow(dead_code))]
    fn cached(
        &mut self,
        font_index: u16,
        ch: char,
        size: f32,
        rasterize: impl FnOnce(f32) -> Glyph,
    ) -> GlyphRef<'_> {
        let key = Key {
            font: font_index,
            size: (size * 4.0) as u16,
            ch,
        };
        self.clock = self.clock.wrapping_add(1);
        let index = match self.entries.iter().position(|e| e.key == key) {
            Some(index) => index,
            None => {
                let glyph = rasterize(f32::from(key.size) / 4.0);
                self.make_room(glyph.byte_size());
                self.bytes += glyph.byte_size();
                self.entries.push(Entry {
                    key,
                    glyph,
                    last_used: 0,
                });
                self.entries.len() - 1
            }
        };
        let entry = &mut self.entries[index];
        entry.last_used = self.clock;
        entry.glyph.as_ref()
    }

    fn make_room(&mut self, needed: usize) {
        while self.bytes + needed > self.capacity && !self.entries.is_empty() {
            let clock = self.clock;
            let oldest = self
                .entries
                .iter()
                .enumerate()
                .max_by_key(|(_, e)| clock.wrapping_sub(e.last_used))
                .map_or(0, |(i, _)| i);
            let removed = self.entries.swap_remove(oldest);
            self.bytes -= removed.glyph.byte_size();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let mut cache = GlyphCache::new(3 * core::mem::size_of::<Glyph>());
        let make = |_| Glyph::empty(1.0);
        cache.cached(0, 'a', 10.0, make);
        cache.cached(0, 'b', 10.0, make);
        cache.cached(0, 'c', 10.0, make);
        cache.cached(0, 'a', 10.0, make); // touch 'a'
        cache.cached(0, 'd', 10.0, make); // evicts 'b'
        let keys: Vec<char> = cache.entries.iter().map(|e| e.key.ch).collect();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&'a') && keys.contains(&'d') && !keys.contains(&'b'));
    }
}

//! Text rendering for QuireInk.
//!
//! - [`Font`]: either a pre-rendered bitmap font pack ([`BitmapFont`], read
//!   in place, works everywhere including the ESP32-C3) or, with the
//!   `outline` feature, a scalable OpenType/TrueType font rendered at
//!   runtime ([`OutlineFont`], via `skrifa`).
//! - A built-in anti-aliasing rasterizer producing 4-bit glyph bitmaps. Packs
//!   are produced with the same rasterizer, so both look identical.
//! - [`GlyphCache`]: a size-bounded LRU cache for runtime-rasterized glyphs.
//! - [`draw_text`], [`measure`] and [`wrap`] for lines and paragraphs.

#![no_std]

extern crate alloc;

mod bitmap;
mod cache;
pub mod charset;
mod font;
mod glyph;
mod layout;
#[cfg(feature = "outline")]
mod outline;
#[cfg_attr(not(feature = "outline"), allow(dead_code))]
mod raster;
pub mod sfnt;

pub use bitmap::{BitmapFont, PackBuilder};
pub use cache::GlyphCache;
pub use font::{Font, FontError, LineMetrics};

/// Whether this build renders scalable (OpenType/TrueType) fonts.
pub const OUTLINES: bool = cfg!(feature = "outline");
pub use glyph::{Glyph, GlyphRef};
pub use layout::{Hyphenate, Line, wrap, wrap_lines, wrap_lines_hyphenated};
#[cfg(feature = "outline")]
pub use outline::OutlineFont;

use quireink_graphics::Canvas;
use quireink_platform::Color;

/// Width of `text` in pixels.
pub fn measure(font: &Font, size: f32, text: &str) -> f32 {
    text.chars().map(|ch| font.advance(ch, size)).sum()
}

/// Draws one line of `text` with its baseline at `baseline` and returns the
/// advance in pixels. Newlines are not interpreted; use [`wrap`] first.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut Canvas<'_>,
    cache: &mut GlyphCache,
    font_index: u16,
    font: &Font,
    size: f32,
    x: f32,
    baseline: i32,
    text: &str,
    color: Color,
) -> f32 {
    draw_text_bolder(
        canvas, cache, font_index, font, size, x, baseline, text, color, 0,
    )
}

/// Like [`draw_text`], with every glyph widened by `extra` pixels to the
/// right: darker, bolder text with any font, also bitmap packs. Advances
/// stay as they are.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_bolder(
    canvas: &mut Canvas<'_>,
    cache: &mut GlyphCache,
    font_index: u16,
    font: &Font,
    size: f32,
    x: f32,
    baseline: i32,
    text: &str,
    color: Color,
    extra: u16,
) -> f32 {
    let mut pen = x;
    for ch in text.chars() {
        let glyph = cache.glyph(font_index, font, ch, size);
        if !glyph.is_empty() {
            let gx = raster::floor(pen + 0.5) + i32::from(glyph.left);
            let gy = baseline + i32::from(glyph.top);
            let width = u32::from(glyph.width);
            canvas.draw_mask(
                gx,
                gy,
                width + u32::from(extra),
                u32::from(glyph.height),
                color,
                |px, py| {
                    // The strongest coverage of the pixels up to `extra`
                    // to the left.
                    (0..=u32::from(extra))
                        .filter(|k| *k <= px && px - k < width)
                        .map(|k| glyph.coverage((px - k) as u16, py as u16))
                        .max()
                        .unwrap_or(0)
                },
            );
        }
        pen += glyph.advance;
    }
    pen - x
}

/// Renders `chars` of `font` at each of `sizes` into a bitmap font pack.
/// Characters the font lacks are skipped.
#[cfg(feature = "outline")]
pub fn build_pack(
    font: &OutlineFont,
    sizes: &[f32],
    chars: impl IntoIterator<Item = char> + Clone,
) -> alloc::vec::Vec<u8> {
    let mut builder = PackBuilder::new();
    for &size in sizes {
        let glyphs = chars
            .clone()
            .into_iter()
            .filter(|&ch| font.has_char(ch))
            .map(|ch| (ch, font.rasterize(ch, size)))
            .collect();
        builder.add_strike(size, font.line_metrics(size), glyphs);
    }
    builder.build()
}

#[cfg(all(test, feature = "outline"))]
mod tests {
    use super::*;

    const FONT: &[u8] =
        include_bytes!("../../../plugins/fonts-default/fonts/AtkinsonHyperlegibleNext.ttf");

    #[test]
    fn packs_render_exactly_like_runtime_rasterization() {
        let outline = OutlineFont::new(FONT, &[("wght", 400.0)]).unwrap();
        let pack = BitmapFont::new(build_pack(&outline, &[24.0], "Agü€?".chars())).unwrap();
        // Pixels are identical; advances are stored in 1/64 px.
        let same = |packed: GlyphRef<'_>, runtime: GlyphRef<'_>| {
            let advance = raster::abs(packed.advance - runtime.advance) <= 1.0 / 128.0;
            let pixels = GlyphRef {
                advance: runtime.advance,
                ..packed
            } == runtime;
            advance && pixels
        };
        for ch in "Agü€".chars() {
            let runtime = outline.rasterize(ch, 24.0);
            assert!(same(pack.glyph(ch, 24.0), runtime.as_ref()), "{ch}");
        }
        // Not in the pack: falls back to '?'.
        let question = outline.rasterize('?', 24.0);
        assert!(same(pack.glyph('Ж', 24.0), question.as_ref()));
    }

    #[test]
    fn rasterizes_visible_glyphs() {
        let font = OutlineFont::new(FONT, &[]).unwrap();
        let glyph = font.rasterize('H', 32.0);
        let r = glyph.as_ref();
        assert!(r.width > 10 && r.height > 15, "{r:?}");
        assert!(r.top < 0, "glyph should sit above the baseline");
        let full = (0..r.height)
            .flat_map(|y| (0..r.width).map(move |x| (x, y)))
            .filter(|&(x, y)| r.coverage(x, y) == 255)
            .count();
        assert!(full > 20, "stems should be fully covered");
        assert!(font.rasterize(' ', 32.0).as_ref().is_empty());
        assert!(font.line_metrics(32.0).line_height > 32.0);
    }
}

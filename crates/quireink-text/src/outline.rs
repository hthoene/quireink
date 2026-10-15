//! Runtime rendering of OpenType/TrueType fonts via `skrifa`.

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

use skrifa::instance::{Location, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::font::{FontError, LineMetrics};
use crate::glyph::{Glyph, pack_coverage};
use crate::raster::{self, Path, Pt};

/// A scalable font, optionally at a fixed position of its variation axes
/// (e.g. weight 700 of a variable font).
///
/// The font data is borrowed for built-in fonts or owned for fonts loaded at
/// runtime.
#[derive(Clone)]
pub struct OutlineFont {
    data: Cow<'static, [u8]>,
    location: Location,
}

impl fmt::Debug for OutlineFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutlineFont")
            .field("bytes", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl OutlineFont {
    /// Loads a font. `variations` sets axes such as `[("wght", 700.0)]`;
    /// unknown axes are ignored.
    pub fn new(
        data: impl Into<Cow<'static, [u8]>>,
        variations: &[(&str, f32)],
    ) -> Result<OutlineFont, FontError> {
        let data = data.into();
        let font = FontRef::new(&data).map_err(|_| FontError::Invalid)?;
        // Any mapping will do: icon fonts have no letters.
        if font.charmap().mappings().next().is_none() {
            return Err(FontError::NoCharmap);
        }
        let location = font.axes().location(variations.iter().copied());
        Ok(OutlineFont { data, location })
    }

    fn font_ref(&self) -> Option<FontRef<'_>> {
        // Validated in `new`; `None` is unreachable in practice.
        FontRef::new(&self.data).ok()
    }

    pub fn line_metrics(&self, size: f32) -> LineMetrics {
        let Some(font) = self.font_ref() else {
            return LineMetrics::fallback(size);
        };
        let metrics = font.metrics(Size::new(size), &self.location);
        let descent = -metrics.descent;
        LineMetrics {
            ascent: metrics.ascent,
            descent,
            line_height: metrics.ascent + descent + metrics.leading,
        }
    }

    pub fn has_char(&self, ch: char) -> bool {
        self.font_ref()
            .is_some_and(|font| font.charmap().map(ch).is_some())
    }

    fn glyph_id(font: &FontRef<'_>, ch: char) -> GlyphId {
        let charmap = font.charmap();
        charmap
            .map(ch)
            .or_else(|| charmap.map('\u{fffd}'))
            .unwrap_or(GlyphId::NOTDEF)
    }

    pub fn advance(&self, ch: char, size: f32) -> f32 {
        let Some(font) = self.font_ref() else {
            return 0.0;
        };
        font.glyph_metrics(Size::new(size), &self.location)
            .advance_width(Self::glyph_id(&font, ch))
            .unwrap_or(0.0)
    }

    /// Rasterizes `ch`; missing characters render as U+FFFD or `.notdef`.
    pub fn rasterize(&self, ch: char, size: f32) -> Glyph {
        let Some(font) = self.font_ref() else {
            return Glyph::empty(0.0);
        };
        let id = Self::glyph_id(&font, ch);
        let advance = font
            .glyph_metrics(Size::new(size), &self.location)
            .advance_width(id)
            .unwrap_or(0.0);
        let mut pen = Pen::default();
        if let Some(outline) = font.outline_glyphs().get(id) {
            let settings = DrawSettings::unhinted(Size::new(size), &self.location);
            // A broken glyph renders as empty instead of failing the page.
            let _ = outline.draw(settings, &mut pen);
        }
        pen.0.close();

        let Some((x0, y0, x1, y1)) = pen.0.bounds() else {
            return Glyph::empty(advance);
        };
        let (width, height) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
        let lines: Vec<(Pt, Pt)> = pen
            .0
            .lines
            .iter()
            .map(|(a, b)| {
                let shift = |p: Pt| Pt {
                    x: p.x - x0 as f32,
                    y: p.y - y0 as f32,
                };
                (shift(*a), shift(*b))
            })
            .collect();
        let coverage = raster::rasterize(&lines, width, height);
        Glyph {
            left: x0 as i16,
            top: y0 as i16,
            width: width as u16,
            height: height as u16,
            advance,
            data: pack_coverage(&coverage, width, height),
        }
    }
}

/// Receives outlines from skrifa (y up) and stores them y-down.
#[derive(Default)]
struct Pen(Path);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(Pt { x, y: -y });
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(Pt { x, y: -y });
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to(Pt { x: cx0, y: -cy0 }, Pt { x, y: -y });
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.cubic_to(
            Pt { x: cx0, y: -cy0 },
            Pt { x: cx1, y: -cy1 },
            Pt { x, y: -y },
        );
    }

    fn close(&mut self) {
        self.0.close();
    }
}

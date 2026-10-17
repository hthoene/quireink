use alloc::vec::Vec;

use quireink_platform::{Color, FrameView, PixelFormat, Rect};

use crate::GraphicsError;
use crate::canvas::Canvas;

/// Pixels in a device's native format, plus dirty-region tracking.
///
/// A framebuffer holds either the whole screen or a *band* of rows
/// ([`Framebuffer::banded`]): the screen is then drawn band by band, each
/// passed to the panel before the next. Coordinates are always screen
/// coordinates; drawing outside the current band is discarded.
#[derive(Debug)]
pub struct Framebuffer {
    width: u16,
    height: u16,
    format: PixelFormat,
    stride: usize,
    /// First screen row held.
    top: u16,
    /// Number of rows held.
    rows: u16,
    data: Vec<u8>,
    dirty: Option<Rect>,
    inverted: bool,
    /// Saturation of RGB pixels drawn, in percent (100: as they are).
    saturation: u16,
}

impl Framebuffer {
    /// Allocates a white framebuffer. Fails instead of aborting when memory
    /// is short, which matters on devices with a few hundred KB of RAM.
    pub fn new(width: u16, height: u16, format: PixelFormat) -> Result<Self, GraphicsError> {
        Self::banded(width, height, height, format)
    }

    /// A framebuffer for a `width`×`height` screen that holds `rows` rows
    /// at a time, starting with the top band.
    pub fn banded(
        width: u16,
        height: u16,
        rows: u16,
        format: PixelFormat,
    ) -> Result<Self, GraphicsError> {
        if format.is_color() && !cfg!(feature = "color") {
            return Err(GraphicsError::UnsupportedFormat(format));
        }
        let rows = rows.clamp(1, height.max(1));
        let stride = format.stride(width);
        let bytes = stride * rows as usize;
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| GraphicsError::OutOfMemory { bytes })?;
        data.resize(bytes, 0xff);
        Ok(Self {
            width,
            height,
            format,
            stride,
            top: 0,
            rows,
            data,
            dirty: None,
            inverted: false,
            saturation: 100,
        })
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// The full screen rectangle.
    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }

    /// Rows held at a time; the screen height unless banded.
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// The part of the screen currently held.
    pub fn band(&self) -> Rect {
        Rect::new(
            0,
            self.top,
            self.width,
            self.rows.min(self.height - self.top),
        )
    }

    /// Moves the band to start at screen row `top`. The pixels are not
    /// cleared; draw the band completely afterwards.
    pub fn set_top(&mut self, top: u16) {
        self.top = top.min(self.height.saturating_sub(1));
    }

    /// Copies `rows` (full-width rows in the same format, starting at
    /// screen row `top`) into this framebuffer, e.g. to keep a copy of the
    /// whole screen from bands. Rows outside the current band are skipped.
    pub fn write_rows(&mut self, top: u16, rows: FrameView<'_>) {
        if rows.format != self.format || rows.width != self.width {
            return;
        }
        let band = self.band();
        for y in 0..rows.height {
            let screen_y = top.saturating_add(y);
            if screen_y < band.y || u32::from(screen_y) >= band.bottom() {
                continue;
            }
            let from = usize::from(y) * rows.stride;
            let to = usize::from(screen_y - self.top) * self.stride;
            let len = self.stride.min(rows.stride);
            if let Some(source) = rows.data.get(from..from + len) {
                self.data[to..to + len].copy_from_slice(source);
            }
        }
    }

    /// Number of bytes used by the pixel data.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    /// The rows held, starting at screen row [`Framebuffer::band`]`.y`.
    pub fn view(&self) -> FrameView<'_> {
        self.view_rows(self.band())
    }

    /// The held rows that `region` covers, as full-width rows starting at
    /// screen row `region.y` (clamped to the band).
    pub fn view_rows(&self, region: Rect) -> FrameView<'_> {
        let band = self.band();
        let first = region.y.clamp(band.y, band.bottom() as u16);
        let last = (region.bottom() as u16).clamp(first, band.bottom() as u16);
        let start = usize::from(first - self.top) * self.stride;
        let end = usize::from(last - self.top) * self.stride;
        FrameView {
            width: self.width,
            height: last - first,
            format: self.format,
            stride: self.stride,
            data: &self.data[start..end],
        }
    }

    /// A canvas covering the current band (the whole screen unless banded).
    pub fn canvas(&mut self) -> Canvas<'_> {
        let band = self.band();
        Canvas::new(self, band)
    }

    /// When inverted, every colour is drawn inverted (night mode). Changing
    /// it does not touch existing pixels; redraw afterwards.
    pub fn set_inverted(&mut self, inverted: bool) {
        self.inverted = inverted;
    }

    pub fn is_inverted(&self) -> bool {
        self.inverted
    }

    /// Draws RGB pixels (pictures) with their saturation at `percent`,
    /// e.g. stronger for colour e-ink, whose filter mutes colours
    /// ([`ColorPanel::boost_percent`](quireink_platform::ColorPanel)).
    /// Grays stay gray.
    pub fn set_saturation(&mut self, percent: u16) {
        self.saturation = percent;
    }

    pub fn saturation(&self) -> u16 {
        self.saturation
    }

    /// Returns and resets the region changed since the last call.
    pub fn take_dirty(&mut self) -> Option<Rect> {
        self.dirty.take()
    }

    pub(crate) fn mark_dirty(&mut self, rect: Rect) {
        self.dirty = Some(match self.dirty {
            Some(dirty) => dirty.union(&rect),
            None => rect,
        });
    }

    /// Fills `rect`, which must lie inside the framebuffer.
    pub(crate) fn fill_clipped(&mut self, rect: Rect, color: Color) {
        if rect.is_empty() {
            return;
        }
        let value = self.encode(color);
        let bpp = self.format.bits_per_pixel();
        // Formats of several pixels a byte: whole bytes at once, the
        // pixels of the bytes at either end one by one (every band is
        // cleared before it is drawn).
        let pattern = match bpp {
            1 => Some(if value & 1 != 0 { 0xFF } else { 0x00 }),
            2 => Some((value as u8 & 0b11) * 0x55),
            4 => Some((value as u8 & 0x0F) * 0x11),
            _ => None,
        };
        let (left, right) = (rect.x, rect.right() as u16);
        for y in rect.y..rect.bottom() as u16 {
            let row = usize::from(y - self.top) * self.stride;
            if self.format == PixelFormat::Gray8 {
                self.data[row + usize::from(left)..row + usize::from(right)].fill(value as u8);
                continue;
            }
            let Some(pattern) = pattern else {
                for x in left..right {
                    self.store(x, y, value);
                }
                continue;
            };
            let per = 8 / u16::from(bpp);
            let first = left.div_ceil(per) * per;
            let last = right / per * per;
            if first >= last {
                for x in left..right {
                    self.store(x, y, value);
                }
                continue;
            }
            for x in left..first {
                self.store(x, y, value);
            }
            self.data[row + usize::from(first / per)..row + usize::from(last / per)].fill(pattern);
            for x in last..right {
                self.store(x, y, value);
            }
        }
        self.mark_dirty(rect);
    }

    /// Blends `color` with coverage `alpha` (0 to 255) over the pixel at
    /// `(x, y)`, which must lie inside the framebuffer. Does not mark the
    /// pixel dirty; callers mark whole areas.
    pub(crate) fn blend_pixel(&mut self, x: u16, y: u16, color: Color, alpha: u8) {
        // Gray formats mix in gray: every glyph's edge comes here, and on
        // small devices the round through red, green and blue is felt.
        if alpha != 255
            && let Some(levels) = self.format.gray_levels()
            && self.format.bits_per_pixel() <= 8
        {
            let max = u32::from(levels - 1);
            let mut under = self.load(x, y) * 255 / max;
            if self.inverted {
                under = 255 - under;
            }
            let (a, over) = (u32::from(alpha), u32::from(color.luma()));
            let mixed = (over * a + under * (255 - a) + 127) / 255;
            let encoded = self.encode(Color::gray(mixed as u8));
            return self.store(x, y, encoded);
        }
        let value = if alpha == 255 {
            color
        } else {
            let stored = self.view().pixel(x, y - self.top);
            let below = if self.inverted {
                stored.inverted()
            } else {
                stored
            };
            let mix = |under: u8, over: u8| {
                let (a, u, o) = (u32::from(alpha), u32::from(under), u32::from(over));
                ((o * a + u * (255 - a) + 127) / 255) as u8
            };
            Color::rgb(
                mix(below.r, color.r),
                mix(below.g, color.g),
                mix(below.b, color.b),
            )
        };
        let encoded = self.encode(value);
        self.store(x, y, encoded);
    }

    /// Sets the pixel at `(x, y)` (inside the band) to `gray`, ordered
    /// dithered for formats with few gray levels, so photos and
    /// illustrations keep their tones.
    pub(crate) fn put_gray(&mut self, x: u16, y: u16, gray: u8) {
        /// 4×4 Bayer matrix.
        const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let gray = match self.format.gray_levels() {
            Some(levels @ 2..=16) => {
                let step = 255.0 / f32::from(levels - 1);
                let threshold = f32::from(BAYER[usize::from(y % 4)][usize::from(x % 4)]);
                let offset = ((threshold + 0.5) / 16.0 - 0.5) * step;
                (f32::from(gray) + offset).clamp(0.0, 255.0) as u8
            }
            _ => gray,
        };
        let value = self.encode(Color::gray(gray));
        self.store(x, y, value);
    }

    /// Sets the pixel at `(x, y)` (inside the band) to `rgb` on colour
    /// formats; on gray ones to its gray, as [`Framebuffer::put_gray`].
    pub(crate) fn put_rgb(&mut self, x: u16, y: u16, rgb: [u8; 3]) {
        if self.format.is_color() {
            let rgb = saturate(rgb, self.saturation);
            let value = self.encode(Color::rgb(rgb[0], rgb[1], rgb[2]));
            self.store(x, y, value);
        } else {
            self.put_gray(x, y, Color::rgb(rgb[0], rgb[1], rgb[2]).luma());
        }
    }

    /// The stored value of a pixel of a format of 8 bits or fewer.
    fn load(&self, x: u16, y: u16) -> u32 {
        let row = usize::from(y - self.top) * self.stride;
        let bpp = usize::from(self.format.bits_per_pixel());
        let bit = usize::from(x) * bpp;
        let byte = self.data[row + bit / 8];
        u32::from(byte >> (8 - bpp - bit % 8)) & ((1 << bpp) - 1)
    }

    /// Writes an encoded pixel value.
    fn store(&mut self, x: u16, y: u16, value: u32) {
        let row = usize::from(y - self.top) * self.stride;
        let x = usize::from(x);
        match self.format.bits_per_pixel() as usize {
            bpp @ (1 | 2 | 4) => {
                let bit = x * bpp;
                let shift = 8 - bpp - bit % 8;
                let mask = (((1u16 << bpp) - 1) as u8) << shift;
                let byte = &mut self.data[row + bit / 8];
                *byte = (*byte & !mask) | (((value as u8) << shift) & mask);
            }
            8 => self.data[row + x] = value as u8,
            16 => {
                let bytes = (value as u16).to_le_bytes();
                self.data[row + x * 2..row + x * 2 + 2].copy_from_slice(&bytes);
            }
            _ => {
                let bytes = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
                self.data[row + x * 3..row + x * 3 + 3].copy_from_slice(&bytes);
            }
        }
    }

    /// Converts a colour to the stored pixel value (nearest level).
    fn encode(&self, color: Color) -> u32 {
        let color = if self.inverted {
            color.inverted()
        } else {
            color
        };
        let quantize = |levels: u32| (color.luma() as u32 * (levels - 1) + 127) / 255;
        match self.format {
            PixelFormat::Mono1 => quantize(2),
            PixelFormat::Gray2 => quantize(4),
            PixelFormat::Gray4 => quantize(16),
            PixelFormat::Gray8 => color.luma() as u32,
            PixelFormat::Rgb565 => {
                let (r, g, b) = (color.r as u32, color.g as u32, color.b as u32);
                (((r * 31 + 127) / 255) << 11)
                    | (((g * 63 + 127) / 255) << 5)
                    | ((b * 31 + 127) / 255)
            }
            PixelFormat::Rgb888 => {
                ((color.r as u32) << 16) | ((color.g as u32) << 8) | color.b as u32
            }
            // Future formats: rejected by `new` until supported.
            _ => 0,
        }
    }
}

/// `rgb` with its distance from its gray scaled to `percent`.
fn saturate(rgb: [u8; 3], percent: u16) -> [u8; 3] {
    if percent == 100 {
        return rgb;
    }
    let luma = i32::from(Color::rgb(rgb[0], rgb[1], rgb[2]).luma());
    rgb.map(|c| (luma + (i32::from(c) - luma) * i32::from(percent) / 100).clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_whole_bytes_and_the_pixels_at_either_end() {
        for format in [PixelFormat::Mono1, PixelFormat::Gray2, PixelFormat::Gray4] {
            for (x, width) in [(0, 37), (3, 1), (5, 2), (7, 25), (9, 30), (16, 16)] {
                let mut fb = Framebuffer::new(40, 3, format).unwrap();
                fb.fill_clipped(Rect::new(x, 1, width, 1), Color::BLACK);
                let view = fb.view();
                for py in 0..3 {
                    for px in 0..40 {
                        let inside = py == 1 && px >= x && px < x + width;
                        let expected = if inside { Color::BLACK } else { Color::WHITE };
                        assert_eq!(
                            view.pixel(px, py),
                            expected,
                            "{format:?} {x}+{width} at ({px}, {py})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn saturation_strengthens_colours_not_grays() {
        assert_eq!(saturate([200, 100, 100], 100), [200, 100, 100]);
        let [r, g, b] = saturate([200, 100, 100], 200);
        assert!(r > 200 && g < 100 && g == b);
        assert_eq!(saturate([90, 90, 90], 200), [90, 90, 90]);
        assert_eq!(saturate([255, 0, 0], 200), [255, 0, 0]);
    }

    fn formats() -> &'static [PixelFormat] {
        if cfg!(feature = "color") {
            &[
                PixelFormat::Mono1,
                PixelFormat::Gray2,
                PixelFormat::Gray4,
                PixelFormat::Gray8,
                PixelFormat::Rgb565,
                PixelFormat::Rgb888,
            ]
        } else {
            &[
                PixelFormat::Mono1,
                PixelFormat::Gray2,
                PixelFormat::Gray4,
                PixelFormat::Gray8,
            ]
        }
    }

    #[test]
    fn bands_hold_their_rows_only() {
        let mut full = Framebuffer::new(8, 10, PixelFormat::Gray2).unwrap();
        let mut band = Framebuffer::banded(8, 10, 4, PixelFormat::Gray2).unwrap();
        assert_eq!(band.byte_len(), full.byte_len() * 4 / 10);
        for top in [0, 4, 8] {
            band.set_top(top);
            let mut canvas = band.canvas();
            canvas.clear(Color::WHITE);
            // A diagonal line across the whole screen.
            for i in 0..10 {
                canvas.set_pixel(quireink_platform::Point::new(i % 8, i), Color::BLACK);
            }
            assert_eq!(band.band().height, if top == 8 { 2 } else { 4 });
            full.write_rows(top, band.view());
        }
        for i in 0..10u16 {
            assert_eq!(full.view().pixel(i % 8, i), Color::BLACK, "row {i}");
            assert_eq!(full.view().pixel((i + 1) % 8, i), Color::WHITE, "row {i}");
        }
        assert_eq!(full.view_rows(Rect::new(0, 3, 8, 2)).height, 2);
    }

    #[test]
    fn starts_white_and_round_trips_black_and_white() {
        for &format in formats() {
            let mut fb = Framebuffer::new(13, 7, format).unwrap();
            assert_eq!(fb.view().pixel(12, 6), Color::WHITE, "{format:?}");
            fb.fill_clipped(Rect::new(3, 2, 5, 3), Color::BLACK);
            let view = fb.view();
            assert_eq!(view.pixel(3, 2), Color::BLACK, "{format:?}");
            assert_eq!(view.pixel(7, 4), Color::BLACK, "{format:?}");
            assert_eq!(view.pixel(8, 4), Color::WHITE, "{format:?}");
            assert_eq!(view.pixel(2, 2), Color::WHITE, "{format:?}");
        }
    }

    #[test]
    fn quantizes_to_nearest_gray_level() {
        let mut fb = Framebuffer::new(4, 1, PixelFormat::Gray2).unwrap();
        fb.fill_clipped(Rect::new(0, 0, 1, 1), Color::gray(80));
        fb.fill_clipped(Rect::new(1, 0, 1, 1), Color::gray(180));
        assert_eq!(fb.view().pixel(0, 0), Color::gray(85));
        assert_eq!(fb.view().pixel(1, 0), Color::gray(170));
    }

    #[test]
    fn blends_over_existing_pixels() {
        let mut fb = Framebuffer::new(2, 1, PixelFormat::Gray8).unwrap();
        fb.blend_pixel(0, 0, Color::BLACK, 128);
        assert_eq!(fb.view().pixel(0, 0), Color::gray(127));
        fb.set_inverted(true);
        fb.fill_clipped(Rect::new(1, 0, 1, 1), Color::WHITE);
        fb.blend_pixel(1, 0, Color::BLACK, 255);
        assert_eq!(fb.view().pixel(1, 0), Color::WHITE);
    }

    #[test]
    fn inversion_applies_to_new_drawing() {
        let mut fb = Framebuffer::new(2, 1, PixelFormat::Mono1).unwrap();
        fb.set_inverted(true);
        fb.fill_clipped(Rect::new(0, 0, 1, 1), Color::WHITE);
        assert_eq!(fb.view().pixel(0, 0), Color::BLACK);
    }

    #[test]
    fn tracks_dirty_region() {
        let mut fb = Framebuffer::new(20, 20, PixelFormat::Gray4).unwrap();
        assert_eq!(fb.take_dirty(), None);
        fb.fill_clipped(Rect::new(1, 1, 2, 2), Color::BLACK);
        fb.fill_clipped(Rect::new(10, 5, 2, 2), Color::BLACK);
        assert_eq!(fb.take_dirty(), Some(Rect::new(1, 1, 11, 6)));
        assert_eq!(fb.take_dirty(), None);
    }

    #[cfg(not(feature = "color"))]
    #[test]
    fn rejects_color_without_feature() {
        assert_eq!(
            Framebuffer::new(1, 1, PixelFormat::Rgb888).unwrap_err(),
            GraphicsError::UnsupportedFormat(PixelFormat::Rgb888)
        );
    }
}

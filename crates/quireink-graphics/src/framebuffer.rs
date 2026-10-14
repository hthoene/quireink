use alloc::vec::Vec;

use quireink_platform::{Color, FrameView, PixelFormat, Rect};

use crate::GraphicsError;
use crate::canvas::Canvas;

/// Pixels in a device's native format, plus dirty-region tracking.
#[derive(Debug)]
pub struct Framebuffer {
    width: u16,
    height: u16,
    format: PixelFormat,
    stride: usize,
    data: Vec<u8>,
    dirty: Option<Rect>,
    inverted: bool,
}

impl Framebuffer {
    /// Allocates a white framebuffer. Fails instead of aborting when memory
    /// is short, which matters on devices with a few hundred KB of RAM.
    pub fn new(width: u16, height: u16, format: PixelFormat) -> Result<Self, GraphicsError> {
        if format.is_color() && !cfg!(feature = "color") {
            return Err(GraphicsError::UnsupportedFormat(format));
        }
        let stride = format.stride(width);
        let bytes = stride * height as usize;
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| GraphicsError::OutOfMemory { bytes })?;
        data.resize(bytes, 0xff);
        Ok(Self {
            width,
            height,
            format,
            stride,
            data,
            dirty: None,
            inverted: false,
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

    /// Number of bytes used by the pixel data.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    pub fn view(&self) -> FrameView<'_> {
        FrameView {
            width: self.width,
            height: self.height,
            format: self.format,
            stride: self.stride,
            data: &self.data,
        }
    }

    /// A canvas covering the whole framebuffer.
    pub fn canvas(&mut self) -> Canvas<'_> {
        let bounds = self.bounds();
        Canvas::new(self, bounds)
    }

    /// When inverted, every colour is drawn inverted (night mode). Changing
    /// it does not touch existing pixels; redraw afterwards.
    pub fn set_inverted(&mut self, inverted: bool) {
        self.inverted = inverted;
    }

    pub fn is_inverted(&self) -> bool {
        self.inverted
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
        let bpp = self.format.bits_per_pixel() as usize;
        for y in rect.y as usize..rect.bottom() as usize {
            let row = y * self.stride;
            match bpp {
                1 | 2 | 4 => {
                    for x in rect.x as usize..rect.right() as usize {
                        let bit = x * bpp;
                        let shift = 8 - bpp - bit % 8;
                        let mask = (((1u16 << bpp) - 1) as u8) << shift;
                        let byte = &mut self.data[row + bit / 8];
                        *byte = (*byte & !mask) | (((value as u8) << shift) & mask);
                    }
                }
                8 => {
                    self.data[row + rect.x as usize..row + rect.right() as usize].fill(value as u8)
                }
                16 => {
                    let bytes = (value as u16).to_le_bytes();
                    for x in rect.x as usize..rect.right() as usize {
                        self.data[row + x * 2..row + x * 2 + 2].copy_from_slice(&bytes);
                    }
                }
                _ => {
                    let bytes = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
                    for x in rect.x as usize..rect.right() as usize {
                        self.data[row + x * 3..row + x * 3 + 3].copy_from_slice(&bytes);
                    }
                }
            }
        }
        self.mark_dirty(rect);
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

#[cfg(test)]
mod tests {
    use super::*;

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

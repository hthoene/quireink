use quireink_platform::{Color, Point, Rect};

use crate::framebuffer::Framebuffer;

/// A drawing surface clipped to a region of a [`Framebuffer`].
///
/// Coordinates are absolute screen coordinates; everything outside the clip
/// region is silently discarded.
#[derive(Debug)]
pub struct Canvas<'a> {
    framebuffer: &'a mut Framebuffer,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    pub(crate) fn new(framebuffer: &'a mut Framebuffer, clip: Rect) -> Self {
        let clip = clip.intersection(&framebuffer.band()).unwrap_or_default();
        Self { framebuffer, clip }
    }

    /// The visible region of this canvas.
    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// A canvas restricted to `region` (intersected with this canvas).
    pub fn clipped(&mut self, region: Rect) -> Canvas<'_> {
        let clip = self.clip.intersection(&region).unwrap_or_default();
        Canvas {
            framebuffer: self.framebuffer,
            clip,
        }
    }

    /// Fills the whole clip region.
    pub fn clear(&mut self, color: Color) {
        self.framebuffer.fill_clipped(self.clip, color);
    }

    /// Fills a rectangle given in signed coordinates.
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        if let Some(rect) = self.clip_signed(x, y, width, height) {
            self.framebuffer.fill_clipped(rect, color);
        }
    }

    /// Draws the outline of a rectangle with the given border thickness,
    /// growing inwards.
    pub fn stroke_rect(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        thickness: u32,
        color: Color,
    ) {
        let t = thickness.min(width / 2 + 1).min(height / 2 + 1);
        self.fill_rect(x, y, width, t, color);
        self.fill_rect(x, y + height as i32 - t as i32, width, t, color);
        self.fill_rect(x, y, t, height, color);
        self.fill_rect(x + width as i32 - t as i32, y, t, height, color);
    }

    pub fn set_pixel(&mut self, point: Point, color: Color) {
        self.fill_rect(point.x, point.y, 1, 1, color);
    }

    /// Blends `color` over a `width`×`height` area at `(x, y)` using
    /// `coverage(dx, dy)` (0 = transparent, 255 = opaque) — used for
    /// anti-aliased glyphs and icons.
    pub fn draw_mask(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        color: Color,
        coverage: impl Fn(u32, u32) -> u8,
    ) {
        let Some(rect) = self.clip_signed(x, y, width, height) else {
            return;
        };
        for py in rect.y..rect.bottom() as u16 {
            for px in rect.x..rect.right() as u16 {
                let alpha = coverage((i32::from(px) - x) as u32, (i32::from(py) - y) as u32);
                if alpha > 0 {
                    self.framebuffer.blend_pixel(px, py, color, alpha);
                }
            }
        }
        self.framebuffer.mark_dirty(rect);
    }

    fn clip_signed(&self, x: i32, y: i32, width: u32, height: u32) -> Option<Rect> {
        let clip = self.clip;
        let left = (x as i64).max(clip.x as i64);
        let top = (y as i64).max(clip.y as i64);
        let right = (x as i64 + width as i64).min(clip.right() as i64);
        let bottom = (y as i64 + height as i64).min(clip.bottom() as i64);
        if right <= left || bottom <= top {
            return None;
        }
        Some(Rect::new(
            left as u16,
            top as u16,
            (right - left) as u16,
            (bottom - top) as u16,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_platform::PixelFormat;

    #[test]
    fn clips_to_screen_and_region() {
        let mut fb = Framebuffer::new(10, 10, PixelFormat::Gray8).unwrap();
        {
            let mut canvas = fb.canvas();
            canvas.fill_rect(-5, -5, 8, 8, Color::BLACK);
        }
        assert_eq!(fb.take_dirty(), Some(Rect::new(0, 0, 3, 3)));

        {
            let mut canvas = fb.canvas();
            let mut inner = canvas.clipped(Rect::new(5, 5, 2, 2));
            inner.clear(Color::BLACK);
            inner.fill_rect(0, 0, 100, 100, Color::BLACK);
        }
        assert_eq!(fb.take_dirty(), Some(Rect::new(5, 5, 2, 2)));
        assert_eq!(fb.view().pixel(4, 4), Color::WHITE);
    }

    #[test]
    fn stroke_draws_only_the_border() {
        let mut fb = Framebuffer::new(10, 10, PixelFormat::Gray8).unwrap();
        fb.canvas().stroke_rect(0, 0, 10, 10, 2, Color::BLACK);
        let view = fb.view();
        assert_eq!(view.pixel(1, 1), Color::BLACK);
        assert_eq!(view.pixel(8, 8), Color::BLACK);
        assert_eq!(view.pixel(2, 2), Color::WHITE);
        assert_eq!(view.pixel(5, 7), Color::WHITE);
    }
}

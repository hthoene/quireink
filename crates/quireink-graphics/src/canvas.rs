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

/// How far row `row` of a shape `height` rows high with corners of
/// `radius` starts in from its sides.
fn corner_inset(radius: u32, row: u32, height: u32) -> u32 {
    let from_edge = row.min(height - 1 - row);
    if from_edge >= radius {
        return 0;
    }
    // The circle's half width at the middle of the row, in half pixels.
    let r = 2 * radius;
    let dy = r - 2 * from_edge - 1;
    let dx = (r * r - dy * dy).isqrt();
    (r - dx).div_ceil(2)
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

    /// Fills a rectangle with corners rounded by `radius` pixels (none
    /// for 0).
    pub fn fill_round_rect(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        radius: u32,
        color: Color,
    ) {
        let r = radius.min(width / 2).min(height / 2);
        if r == 0 {
            return self.fill_rect(x, y, width, height, color);
        }
        for row in 0..height {
            let inset = corner_inset(r, row, height);
            self.fill_rect(
                x + inset as i32,
                y + row as i32,
                width - 2 * inset,
                1,
                color,
            );
        }
    }

    /// Draws the outline of a rectangle with corners rounded by `radius`
    /// pixels, `thickness` wide, growing inwards.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_round_rect(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        thickness: u32,
        radius: u32,
        color: Color,
    ) {
        let r = radius.min(width / 2).min(height / 2);
        let t = thickness.min(width / 2 + 1).min(height / 2 + 1);
        if r == 0 {
            return self.stroke_rect(x, y, width, height, thickness, color);
        }
        // Each row: the outer span, less the inner rectangle's span.
        let (inner_w, inner_h) = (width.saturating_sub(2 * t), height.saturating_sub(2 * t));
        let inner_r = r.saturating_sub(t);
        for row in 0..height {
            let outer = corner_inset(r, row, height);
            let left = x + outer as i32;
            let span = width - 2 * outer;
            let inner_row = row.checked_sub(t).filter(|r| *r < inner_h);
            match inner_row {
                Some(inner_row) if inner_w > 0 => {
                    let inner = t + corner_inset(inner_r, inner_row, inner_h);
                    let side = inner.saturating_sub(outer);
                    self.fill_rect(left, y + row as i32, side, 1, color);
                    let right = x + (width - inner) as i32;
                    self.fill_rect(right, y + row as i32, side, 1, color);
                }
                _ => self.fill_rect(left, y + row as i32, span, 1, color),
            }
        }
    }

    pub fn set_pixel(&mut self, point: Point, color: Color) {
        self.fill_rect(point.x, point.y, 1, 1, color);
    }

    /// Draws a row of 8-bit gray pixels (an image row) starting at
    /// `(x, y)`; dithered on panels with few gray levels.
    pub fn draw_gray_row(&mut self, x: i32, y: i32, gray: &[u8]) {
        let Some(rect) = self.clip_signed(x, y, gray.len() as u32, 1) else {
            return;
        };
        for px in rect.x..rect.right() as u16 {
            let value = gray[(i32::from(px) - x) as usize];
            self.framebuffer.put_gray(px, rect.y, value);
        }
        self.framebuffer.mark_dirty(rect);
    }

    /// Blends `color` over a `width`×`height` area at `(x, y)` using
    /// `coverage(dx, dy)` (0 = transparent, 255 = opaque), used for
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
    fn round_corners_leave_the_corners() {
        let mut fb = Framebuffer::new(20, 12, PixelFormat::Gray8).unwrap();
        fb.canvas().fill_round_rect(0, 0, 20, 12, 4, Color::BLACK);
        let gray = |x, y| fb.view().pixel(x, y).r;
        // The corners stay white, the middle and the straight edges fill.
        assert_eq!(gray(0, 0), 255);
        assert_eq!(gray(19, 11), 255);
        assert_eq!(gray(10, 0), 0);
        assert_eq!(gray(0, 6), 0);
        assert_eq!(gray(10, 6), 0);
    }

    #[test]
    fn round_outlines_are_hollow() {
        let mut fb = Framebuffer::new(20, 12, PixelFormat::Gray8).unwrap();
        fb.canvas().stroke_round_rect(0, 0, 20, 12, 2, 4, Color::BLACK);
        let gray = |x, y| fb.view().pixel(x, y).r;
        assert_eq!(gray(0, 0), 255);
        assert_eq!(gray(10, 0), 0);
        assert_eq!(gray(0, 6), 0);
        assert_eq!(gray(10, 6), 255);
        // Without a radius, as stroke_rect.
        let mut square = Framebuffer::new(6, 6, PixelFormat::Gray8).unwrap();
        square.canvas().stroke_round_rect(0, 0, 6, 6, 1, 0, Color::BLACK);
        assert_eq!(square.view().pixel(0, 0).r, 0);
    }

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

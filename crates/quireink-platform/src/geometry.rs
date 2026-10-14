//! Screen-space geometry.

/// A point in logical screen coordinates.
///
/// Coordinates are signed so that drawing code can address positions outside
/// the visible area; they are clipped when drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle of screen pixels.
///
/// Rectangles in screen space are always non-negative; the largest supported
/// screen dimension is `u16::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// A rectangle covering a full screen of the given size.
    pub const fn from_size(width: u16, height: u16) -> Self {
        Self::new(0, 0, width, height)
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Exclusive right edge.
    pub const fn right(&self) -> u32 {
        self.x as u32 + self.width as u32
    }

    /// Exclusive bottom edge.
    pub const fn bottom(&self) -> u32 {
        self.y as u32 + self.height as u32
    }

    pub const fn area(&self) -> u32 {
        self.width as u32 * self.height as u32
    }

    pub const fn contains(&self, point: Point) -> bool {
        point.x >= self.x as i32
            && point.y >= self.y as i32
            && (point.x as i64) < self.right() as i64
            && (point.y as i64) < self.bottom() as i64
    }

    /// The smallest rectangle containing both `self` and `other`.
    ///
    /// Empty rectangles are ignored.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect::new(x, y, (right - x as u32) as u16, (bottom - y as u32) as u16)
    }

    /// The overlapping part of `self` and `other`, if any.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x as u32 || bottom <= y as u32 {
            return None;
        }
        Some(Rect::new(
            x,
            y,
            (right - x as u32) as u16,
            (bottom - y as u32) as u16,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_ignores_empty() {
        let a = Rect::new(10, 10, 5, 5);
        assert_eq!(a.union(&Rect::default()), a);
        assert_eq!(Rect::default().union(&a), a);
    }

    #[test]
    fn union_covers_both() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(20, 5, 10, 10);
        assert_eq!(a.union(&b), Rect::new(0, 0, 30, 15));
    }

    #[test]
    fn intersection() {
        let a = Rect::new(0, 0, 10, 10);
        assert_eq!(
            a.intersection(&Rect::new(5, 5, 10, 10)),
            Some(Rect::new(5, 5, 5, 5))
        );
        assert_eq!(a.intersection(&Rect::new(10, 0, 5, 5)), None);
    }

    #[test]
    fn contains_is_exclusive_at_far_edges() {
        let r = Rect::new(2, 2, 3, 3);
        assert!(r.contains(Point::new(2, 2)));
        assert!(r.contains(Point::new(4, 4)));
        assert!(!r.contains(Point::new(5, 4)));
        assert!(!r.contains(Point::new(-1, 3)));
    }
}

//! An anti-aliasing scanline rasterizer for glyph outlines.
//!
//! Uses signed-area accumulation (the approach popularised by `font-rs`):
//! every line segment adds its exact area contribution to an accumulation
//! buffer, and a running sum per row yields the coverage of each pixel.
//! Curves are flattened into line segments first.

use alloc::vec;
use alloc::vec::Vec;

/// A point in pixel space, y pointing down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pt {
    pub x: f32,
    pub y: f32,
}

/// Collects an outline as flattened line segments.
#[derive(Debug, Default)]
pub(crate) struct Path {
    pub lines: Vec<(Pt, Pt)>,
    start: Option<Pt>,
    current: Option<Pt>,
}

impl Path {
    pub fn move_to(&mut self, p: Pt) {
        self.close();
        self.start = Some(p);
        self.current = Some(p);
    }

    pub fn line_to(&mut self, p: Pt) {
        if let Some(c) = self.current {
            self.lines.push((c, p));
        }
        self.current = Some(p);
    }

    pub fn quad_to(&mut self, c: Pt, p: Pt) {
        let Some(p0) = self.current else { return };
        let n = segments(manhattan(p0, c) + manhattan(c, p));
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            self.line_to(Pt {
                x: mt * mt * p0.x + 2.0 * mt * t * c.x + t * t * p.x,
                y: mt * mt * p0.y + 2.0 * mt * t * c.y + t * t * p.y,
            });
        }
    }

    pub fn cubic_to(&mut self, c0: Pt, c1: Pt, p: Pt) {
        let Some(p0) = self.current else { return };
        let n = segments(manhattan(p0, c0) + manhattan(c0, c1) + manhattan(c1, p));
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let (a, b, c, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
            self.line_to(Pt {
                x: a * p0.x + b * c0.x + c * c1.x + d * p.x,
                y: a * p0.y + b * c0.y + c * c1.y + d * p.y,
            });
        }
    }

    pub fn close(&mut self) {
        if let (Some(s), Some(c)) = (self.start, self.current)
            && s != c
        {
            self.lines.push((c, s));
        }
        self.current = self.start;
    }

    /// Integer pixel bounds `(x0, y0, x1, y1)` covering all segments.
    pub fn bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut points = self.lines.iter().flat_map(|(a, b)| [*a, *b]);
        let first = points.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Some((floor(x0), floor(y0), ceil(x1), ceil(y1)))
    }
}

fn manhattan(a: Pt, b: Pt) -> f32 {
    abs(a.x - b.x) + abs(a.y - b.y)
}

/// Segment count for a curve whose control polygon has length `len`.
fn segments(len: f32) -> usize {
    (ceil(len / 3.0).max(1) as usize).min(32)
}

/// Rasterizes `lines` (already translated so the bounding box starts at
/// 0,0) into an 8-bit coverage bitmap of `width`×`height`.
pub(crate) fn rasterize(lines: &[(Pt, Pt)], width: usize, height: usize) -> Vec<u8> {
    // Two extra cells absorb contributions right of the last column.
    let mut acc = vec![0.0f32; width * height + 2];
    for &(p0, p1) in lines {
        accumulate_line(&mut acc, width, height, p0, p1);
    }
    // One running sum over the whole buffer: an edge on a row's right
    // border contributes to the next cell, which is the start of the next
    // row, where it cancels exactly.
    let mut sum = 0.0f32;
    acc[..width * height]
        .iter()
        .map(|a| {
            sum += a;
            (abs(sum).min(1.0) * 255.0 + 0.5) as u8
        })
        .collect()
}

fn accumulate_line(acc: &mut [f32], width: usize, height: usize, p0: Pt, p1: Pt) {
    if p0.y == p1.y {
        return;
    }
    let (dir, p0, p1) = if p0.y < p1.y {
        (1.0, p0, p1)
    } else {
        (-1.0, p1, p0)
    };
    let dxdy = (p1.x - p0.x) / (p1.y - p0.y);
    let mut x = p0.x;
    if p0.y < 0.0 {
        x -= p0.y * dxdy;
    }
    let y_start = floor(p0.y.max(0.0)) as usize;
    let y_end = (ceil(p1.y).max(0) as usize).min(height);
    let max_index = acc.len() as i64 - 1;
    let mut add = |index: i64, value: f32| {
        acc[index.clamp(0, max_index) as usize] += value;
    };
    for y in y_start..y_end {
        let row = (y * width) as i64;
        let dy = ((y + 1) as f32).min(p1.y) - (y as f32).max(p0.y);
        let x_next = x + dxdy * dy;
        let d = dy * dir;
        let (x0, x1) = if x < x_next { (x, x_next) } else { (x_next, x) };
        let x0_floor = floor(x0) as f32;
        let x0i = x0_floor as i64;
        let x1_ceil = ceil(x1) as f32;
        let x1i = x1_ceil as i64;
        if x1i <= x0i + 1 {
            let xmf = 0.5 * (x + x_next) - x0_floor;
            add(row + x0i, d - d * xmf);
            add(row + x0i + 1, d * xmf);
        } else {
            let s = 1.0 / (x1 - x0);
            let x0f = x0 - x0_floor;
            let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
            let x1f = x1 - x1_ceil + 1.0;
            let am = 0.5 * s * x1f * x1f;
            add(row + x0i, d * a0);
            if x1i == x0i + 2 {
                add(row + x0i + 1, d * (1.0 - a0 - am));
            } else {
                let a1 = s * (1.5 - x0f);
                add(row + x0i + 1, d * (a1 - a0));
                for xi in x0i + 2..x1i - 1 {
                    add(row + xi, d * s);
                }
                let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                add(row + x1i - 1, d * (1.0 - a2 - am));
            }
            add(row + x1i, d * am);
        }
        x = x_next;
    }
}

// Float helpers that `core` does not provide without `std`.

pub(crate) fn floor(x: f32) -> i32 {
    let i = x as i32;
    if (i as f32) > x { i - 1 } else { i }
}

pub(crate) fn ceil(x: f32) -> i32 {
    let i = x as i32;
    if (i as f32) < x { i + 1 } else { i }
}

pub(crate) fn abs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(path: &mut Path, x0: f32, y0: f32, x1: f32, y1: f32) {
        path.move_to(Pt { x: x0, y: y0 });
        path.line_to(Pt { x: x1, y: y0 });
        path.line_to(Pt { x: x1, y: y1 });
        path.line_to(Pt { x: x0, y: y1 });
        path.close();
    }

    #[test]
    fn pixel_aligned_square_is_fully_covered() {
        let mut path = Path::default();
        square(&mut path, 1.0, 1.0, 3.0, 3.0);
        let c = rasterize(&path.lines, 4, 4);
        assert_eq!(c[5], 255);
        assert_eq!(c[10], 255);
        assert_eq!(c[0], 0);
        assert_eq!(c[15], 0);
    }

    #[test]
    fn half_pixel_edges_give_half_coverage() {
        let mut path = Path::default();
        square(&mut path, 0.5, 0.0, 2.0, 1.0);
        let c = rasterize(&path.lines, 2, 1);
        assert!((126..=129).contains(&c[0]), "{}", c[0]);
        assert_eq!(c[1], 255);
    }

    #[test]
    fn winding_direction_does_not_matter() {
        let mut path = Path::default();
        path.move_to(Pt { x: 0.0, y: 0.0 });
        path.line_to(Pt { x: 0.0, y: 2.0 });
        path.line_to(Pt { x: 2.0, y: 2.0 });
        path.line_to(Pt { x: 2.0, y: 0.0 });
        path.close();
        assert!(rasterize(&path.lines, 2, 2).iter().all(|&v| v == 255));
    }

    #[test]
    fn float_helpers() {
        assert_eq!(floor(-0.5), -1);
        assert_eq!(floor(1.5), 1);
        assert_eq!(ceil(1.2), 2);
        assert_eq!(ceil(-1.2), -1);
        assert_eq!(ceil(2.0), 2);
    }
}

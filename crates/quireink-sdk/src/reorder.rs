//! Moving rows of a [`ListView`] by dragging, as on phones: movable rows
//! carry a grip (≡) at their end; a finger that comes down on it takes the
//! row along, a bar shows where it will land, and lifting the finger puts
//! it there. Taps on the rest of the row work as before, and buttons keep
//! their own ways of moving rows (e.g. *Move up*, *Move down*).
//!
//! A screen keeps a [`Reorder`], draws rows with [`Reorder::draw_grip`]
//! in what [`Reorder::content`] leaves, the bar with [`Reorder::draw`],
//! and passes pointer samples to [`Reorder::on_pointer`] from
//! [`Screen::on_pointer`](crate::Screen::on_pointer).

use alloc::vec::Vec;

use crate::semantics::Role;
use crate::ui::{ListView, UiStyle, draw_icon, fill, icon_width};
use crate::{Canvas, Color, Env, PointerEvent, PointerPhase, Rect};

/// The grip's icon.
const GRIP: &str = "menu-2";

/// Rows being moved by dragging; see the [module documentation](self).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reorder {
    drag: Option<Drag>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drag {
    pointer: u8,
    from: usize,
    to: usize,
}

/// What a pointer sample did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReorderStep {
    /// A row was taken, or the place it would land changed: draw again.
    Changed,
    /// Nothing to draw (the finger moved within the same place).
    Same,
    /// The row was let go: move the item at `from` to `to` (see [`moved`]).
    Dropped { from: usize, to: usize },
}

impl Reorder {
    /// The grip of `row`: as wide as the row is high, at its end.
    pub fn grip(row: Rect) -> Rect {
        let width = row.height.min(row.width);
        Rect::new(row.x + row.width - width, row.y, width, row.height)
    }

    /// What is left of `row` for its content when it has a grip.
    pub fn content(row: Rect) -> Rect {
        Rect::new(row.x, row.y, row.width - Self::grip(row).width, row.height)
    }

    /// Draws the grip of `row` (white on a focused row).
    pub fn draw_grip(
        canvas: &mut Canvas<'_>,
        env: &Env<'_>,
        style: &UiStyle,
        row: Rect,
        focused: bool,
    ) {
        let grip = Self::grip(row);
        let size = style.text;
        // Flush with the margin, where other rows end their values.
        let right = f32::from(grip.x + grip.width) - style.margin as f32;
        let x = right - icon_width(env, GRIP, size);
        let ink = if focused { Color::WHITE } else { Color::BLACK };
        canvas.describe(Role::Button, "", grip, false, None);
        draw_icon(canvas, env, GRIP, size, x, grip, ink);
    }

    /// The row being moved and where it would land, while one is.
    pub fn dragging(&self) -> Option<(usize, usize)> {
        self.drag.map(|d| (d.from, d.to))
    }

    /// Follows a finger: one that comes down on the grip of one of the
    /// first `movable` items takes it along. `None` for samples it does
    /// not take: the screen then handles them as it would.
    pub fn on_pointer(
        &mut self,
        event: &PointerEvent,
        list: &ListView,
        movable: usize,
    ) -> Option<ReorderStep> {
        match (event.phase, self.drag) {
            (PointerPhase::Down, None) => {
                let index = list.hit(event.point, movable)?;
                let slot = index - list.page() * list.rows();
                if !Self::grip(list.row_rect(slot)).contains(event.point) {
                    return None;
                }
                self.drag = Some(Drag {
                    pointer: event.id,
                    from: index,
                    to: index,
                });
                Some(ReorderStep::Changed)
            }
            (PointerPhase::Move, Some(mut drag)) if drag.pointer == event.id => {
                let to = Self::place(event, list, movable);
                if to == drag.to {
                    return Some(ReorderStep::Same);
                }
                drag.to = to;
                self.drag = Some(drag);
                Some(ReorderStep::Changed)
            }
            (PointerPhase::Up, Some(drag)) if drag.pointer == event.id => {
                self.drag = None;
                let to = Self::place(event, list, movable);
                Some(ReorderStep::Dropped {
                    from: drag.from,
                    to,
                })
            }
            (PointerPhase::Cancel, Some(drag)) if drag.pointer == event.id => {
                self.drag = None;
                Some(ReorderStep::Changed)
            }
            _ => None,
        }
    }

    /// Where a row would land with the finger at the event: the visible
    /// row under it, kept to the movable ones on the page.
    fn place(event: &PointerEvent, list: &ListView, movable: usize) -> usize {
        let first = list.page() * list.rows();
        let last = (first + list.rows()).min(movable).max(first + 1) - 1;
        let y = event.point.y - i32::from(list.area.y);
        let slot = (y.max(0) as u32 / list.row) as usize;
        (first + slot).clamp(first, last)
    }

    /// Draws, while a row is moved, a frame around it and a bar where it
    /// would land.
    pub fn draw(&self, canvas: &mut Canvas<'_>, style: &UiStyle, list: &ListView) {
        let Some(Drag { from, to, .. }) = self.drag else {
            return;
        };
        let first = list.page() * list.rows();
        if let Some(slot) = from.checked_sub(first).filter(|&s| s < list.rows()) {
            let r = list.row_rect(slot);
            canvas.stroke_rect(
                i32::from(r.x),
                i32::from(r.y),
                u32::from(r.width),
                u32::from(r.height),
                style.line * 2,
                Color::BLACK,
            );
        }
        if to == from {
            return;
        }
        let r = list.row_rect(to - first);
        // Below the row it passes when moving down, above it when up.
        let thick = (style.line * 4) as u16;
        let y = if to > from {
            r.y + r.height - thick / 2
        } else {
            r.y.saturating_sub(thick / 2)
        };
        let margin = style.margin as u16;
        let bar = Rect::new(r.x + margin, y, r.width.saturating_sub(2 * margin), thick);
        fill(canvas, bar, Color::BLACK);
    }
}

/// `items` with the one at `from` moved to `to`, the others keeping their
/// order.
pub fn moved<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from < items.len() && to < items.len() && from != to {
        let item = items.remove(from);
        items.insert(to, item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Point, PointerTool};

    fn event(phase: PointerPhase, x: i32, y: i32) -> PointerEvent {
        PointerEvent {
            id: 0,
            phase,
            point: Point::new(x, y),
            tool: PointerTool::Finger,
            pressure: 1000,
        }
    }

    #[test]
    fn rows_are_dragged_by_their_grip() {
        // Rows 100 high in 600: 6 a page, the grip at x 900..1000.
        let list = ListView::new(Rect::new(0, 100, 1000, 600), 100);
        let mut reorder = Reorder::default();
        // Not on the grip, or not a movable row: not taken.
        assert_eq!(
            reorder.on_pointer(&event(PointerPhase::Down, 500, 150), &list, 4),
            None
        );
        assert_eq!(
            reorder.on_pointer(&event(PointerPhase::Down, 950, 550), &list, 4),
            None
        );
        let step = reorder.on_pointer(&event(PointerPhase::Down, 950, 150), &list, 4);
        assert_eq!(step, Some(ReorderStep::Changed));
        assert_eq!(reorder.dragging(), Some((0, 0)));
        let step = reorder.on_pointer(&event(PointerPhase::Move, 950, 260), &list, 4);
        assert_eq!(step, Some(ReorderStep::Changed));
        let step = reorder.on_pointer(&event(PointerPhase::Move, 950, 280), &list, 4);
        assert_eq!(step, Some(ReorderStep::Same));
        // Below the movable rows: the last of them.
        let step = reorder.on_pointer(&event(PointerPhase::Up, 950, 690), &list, 4);
        assert_eq!(step, Some(ReorderStep::Dropped { from: 0, to: 3 }));
        assert_eq!(reorder.dragging(), None);

        let mut items = alloc::vec![1, 2, 3, 4];
        moved(&mut items, 0, 3);
        assert_eq!(items, [2, 3, 4, 1]);
        moved(&mut items, 3, 1);
        assert_eq!(items, [2, 1, 3, 4]);
    }
}

//! Rows of different heights on pages (see [`Rows`]): entries and the headings of
//! sections, which take less room and are never focused. A section that
//! goes on over a page break has its heading again at the top of the next
//! page.

use alloc::vec::Vec;
use core::ops::Range;

use crate::ui::{ListChange, UiStyle, fill};
use crate::{Canvas, Color, Point, Rect};

#[derive(Debug)]
pub struct Rows {
    pub area: Rect,
    heights: Vec<u16>,
    headings: Vec<bool>,
    /// The rows of each page, and the heading repeated at its top.
    pages: Vec<(Option<usize>, Range<usize>)>,
    /// The focused row; never a heading while there are entries.
    pub focus: usize,
}

impl Rows {
    /// Rows of `heights` in `area`; `headings` marks the headings.
    pub fn new(area: Rect, heights: Vec<u16>, headings: Vec<bool>) -> Self {
        let mut pages: Vec<(Option<usize>, Range<usize>)> = Vec::new();
        let mut start = 0;
        let mut carried: Option<usize> = None;
        let mut used = 0u16;
        for (i, &height) in heights.iter().enumerate() {
            if i > start && used + height > area.height {
                // A heading never ends a page: it goes with its entries.
                let end = if headings[i - 1] && i - 1 > start {
                    i - 1
                } else {
                    i
                };
                pages.push((carried, start..end));
                // The section going on: its heading again.
                carried = (!headings[end])
                    .then(|| (0..end).rev().find(|&h| headings[h]))
                    .flatten();
                used = carried.map_or(0, |h| heights[h]) + heights[end..i].iter().sum::<u16>();
                start = end;
            }
            used += height;
        }
        pages.push((carried, start..heights.len()));
        let focus = headings.iter().position(|h| !h).unwrap_or(0);
        Self {
            area,
            heights,
            headings,
            pages,
            focus,
        }
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// The page holding row `index`.
    fn page_of(&self, index: usize) -> usize {
        self.pages
            .iter()
            .position(|(_, rows)| rows.contains(&index))
            .unwrap_or(0)
    }

    /// The page with the focus, and the number of pages.
    pub fn paging(&self) -> (usize, usize) {
        (self.page_of(self.focus), self.pages.len())
    }

    /// The rows of the page with the focus and where they are.
    pub fn visible(&self) -> Vec<(usize, Rect)> {
        let (carried, rows) = self.pages[self.page_of(self.focus)].clone();
        let mut y = self.area.y;
        carried
            .into_iter()
            .chain(rows)
            .map(|i| {
                let rect = Rect::new(self.area.x, y, self.area.width, self.heights[i]);
                y += self.heights[i];
                (i, rect)
            })
            .collect()
    }

    pub fn rect_of(&self, index: usize) -> Option<Rect> {
        self.visible()
            .into_iter()
            .find(|(i, _)| *i == index)
            .map(|(_, rect)| rect)
    }

    pub fn is_heading(&self, index: usize) -> bool {
        self.headings.get(index).copied().unwrap_or(false)
    }

    /// Draws the rows of the focused page with `draw_row(canvas, index,
    /// rect, focused)`, headings included: the focused entry (if `focus`
    /// shows) on black, thin lines between entries but not above a
    /// heading.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        style: &UiStyle,
        focus: bool,
        mut draw_row: impl FnMut(&mut Canvas<'_>, usize, Rect, bool),
    ) {
        let visible = self.visible();
        for (n, &(index, rect)) in visible.iter().enumerate() {
            let focused = focus && index == self.focus && !self.is_heading(index);
            if focused {
                crate::ui::fill_round(canvas, style, rect, Color::BLACK);
            } else if !self.is_heading(index)
                && visible
                    .get(n + 1)
                    .is_some_and(|&(next, _)| !self.is_heading(next))
            {
                let line = Rect::new(
                    rect.x + style.margin as u16,
                    rect.y + rect.height - style.line as u16,
                    rect.width.saturating_sub(2 * style.margin as u16),
                    style.line as u16,
                );
                fill(canvas, line, Color::gray(190));
            }
            draw_row(canvas, index, rect, focused);
        }
    }

    /// The entry at `point`, if any (headings are not hit).
    pub fn hit(&self, point: Point) -> Option<usize> {
        self.visible()
            .into_iter()
            .find(|(i, rect)| rect.contains(point) && !self.is_heading(*i))
            .map(|(i, _)| i)
    }

    fn set_focus(&mut self, focus: usize) -> ListChange {
        if focus == self.focus {
            return ListChange::None;
        }
        let page = self.page_of(self.focus);
        self.focus = focus;
        if self.page_of(focus) == page {
            ListChange::Focus
        } else {
            ListChange::Page
        }
    }

    /// Moves the focus to the next (`forward`) or previous entry.
    pub fn step(&mut self, forward: bool) -> ListChange {
        let next = if forward {
            (self.focus + 1..self.len()).find(|&i| !self.is_heading(i))
        } else {
            (0..self.focus).rev().find(|&i| !self.is_heading(i))
        };
        next.map_or(ListChange::None, |i| self.set_focus(i))
    }

    /// Turns to the next (`forward`) or previous page, focusing its
    /// first entry.
    pub fn turn(&mut self, forward: bool) -> ListChange {
        let page = self.page_of(self.focus);
        let target = if forward {
            page + 1
        } else {
            match page.checked_sub(1) {
                Some(page) => page,
                None => return ListChange::None,
            }
        };
        let Some((_, range)) = self.pages.get(target).cloned() else {
            return ListChange::None;
        };
        match range.clone().find(|&i| !self.is_heading(i)) {
            Some(first) => self.set_focus(first),
            None => ListChange::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_go_with_their_entries() {
        // Heading, two entries, heading, two entries; three rows a page.
        let heights = Vec::from([5, 10, 10, 5, 10, 10]);
        let headings = Vec::from([true, false, false, true, false, false]);
        let mut rows = Rows::new(Rect::new(0, 0, 100, 30), heights, headings);
        assert_eq!(rows.pages, [(None, 0..3), (None, 3..6)]);
        assert_eq!(rows.focus, 1, "the first entry");
        assert_eq!(rows.step(true), ListChange::Focus);
        assert_eq!(rows.step(true), ListChange::Page, "over the heading");
        assert_eq!(rows.focus, 4);
        assert_eq!(rows.paging(), (1, 2));
        assert_eq!(rows.turn(false), ListChange::Page);
        assert_eq!(rows.focus, 1);
        let visible = rows.visible();
        assert_eq!(visible[0].1, Rect::new(0, 0, 100, 5));
        assert_eq!(visible[1].1, Rect::new(0, 5, 100, 10));
        assert_eq!(rows.hit(Point { x: 3, y: 2 }), None, "a heading");
        assert_eq!(rows.hit(Point { x: 3, y: 7 }), Some(1));
    }

    #[test]
    fn a_heading_never_ends_a_page() {
        let heights = Vec::from([10, 10, 5, 10]);
        let headings = Vec::from([false, false, true, false]);
        let rows = Rows::new(Rect::new(0, 0, 100, 25), heights, headings);
        assert_eq!(rows.pages, [(None, 0..2), (None, 2..4)]);
    }

    #[test]
    fn a_section_going_on_repeats_its_heading() {
        // Heading, four entries; two entries fit a page with the heading.
        let heights = Vec::from([5, 10, 10, 10, 10]);
        let headings = Vec::from([true, false, false, false, false]);
        let mut rows = Rows::new(Rect::new(0, 0, 100, 25), heights, headings);
        assert_eq!(rows.pages, [(None, 0..3), (Some(0), 3..5)]);
        rows.turn(true);
        let visible: Vec<usize> = rows.visible().into_iter().map(|(i, _)| i).collect();
        assert_eq!(visible, [0, 3, 4], "the heading first");
        assert_eq!(rows.focus, 3);
    }
}

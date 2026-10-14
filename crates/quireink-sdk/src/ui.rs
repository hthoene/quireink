//! Building blocks for plugin user interfaces.
//!
//! All sizes derive from the device's density and the `core.ui-size`
//! setting, so every plugin's UI looks consistent and scales together when
//! the user chooses larger text.

use alloc::string::{String, ToString};

use quireink_graphics::Canvas;
use quireink_platform::{Color, Point, Rect};

use crate::builtin::{fonts, settings};
use crate::env::Env;
use crate::fonts::FontHandle;

/// Sizes of the standard UI elements, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiStyle {
    /// Body text size.
    pub text: f32,
    /// Secondary text (status lines, hints).
    pub small: f32,
    /// Header titles.
    pub title: f32,
    /// Height of a list row.
    pub row: u32,
    /// Height of a header bar.
    pub header: u32,
    /// Horizontal margin of content.
    pub margin: u32,
    /// Standard gap between elements.
    pub gap: u32,
    /// Width of lines and borders.
    pub line: u32,
}

impl UiStyle {
    pub fn new(env: &Env<'_>) -> Self {
        let m = env.metrics;
        let base = env.settings.int(settings::UI_SIZE.as_str()).clamp(6, 40) as f32;
        let text = m.pt(base);
        let line_height = env
            .fonts
            .role(fonts::UI.as_str())
            .map_or(text * 1.25, |f| f.line_metrics(text).line_height);
        Self {
            text,
            small: m.pt(base * 0.75),
            title: m.pt(base * 1.15),
            row: (line_height * 1.8) as u32,
            header: (line_height * 1.9) as u32,
            margin: m.mm(3),
            gap: m.mm(2),
            line: m.tenth_mm(3),
        }
    }

    /// The UI font.
    pub fn font<'a>(&self, env: &Env<'a>) -> Option<FontHandle<'a>> {
        env.fonts.role(fonts::UI.as_str())
    }
}

/// Draws `text` vertically centred in `area`, starting at `x`.
pub fn draw_label(
    canvas: &mut Canvas<'_>,
    font: &FontHandle<'_>,
    size: f32,
    x: f32,
    area: Rect,
    text: &str,
    color: Color,
) -> f32 {
    let metrics = font.line_metrics(size);
    let baseline = i32::from(area.y)
        + ((f32::from(area.height) + metrics.ascent - metrics.descent) / 2.0) as i32;
    font.draw(canvas, size, x, baseline, text, color)
}

/// A black header bar with a title and an optional right-aligned note.
pub fn draw_header(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    area: Rect,
    title: &str,
    note: Option<&str>,
) {
    fill(canvas, area, Color::BLACK);
    let Some(font) = style.font(env) else {
        return;
    };
    let mut canvas = canvas.clipped(area);
    let margin = style.margin as f32;
    let mut right = f32::from(area.x) + f32::from(area.width) - margin;
    if let Some(note) = note {
        right -= font.measure(style.small, note);
        draw_label(
            &mut canvas,
            &font,
            style.small,
            right,
            area,
            note,
            Color::WHITE,
        );
        right -= margin;
    }
    let title = fit(
        &font,
        style.title,
        title,
        right - f32::from(area.x) - margin,
    );
    draw_label(
        &mut canvas,
        &font,
        style.title,
        f32::from(area.x) + margin,
        area,
        &title,
        Color::WHITE,
    );
}

/// Draws wrapped, centred message text in `area`.
pub fn draw_message(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    area: Rect,
    text: &str,
) {
    let Some(font) = style.font(env) else {
        return;
    };
    let width = f32::from(area.width) - 2.0 * style.margin as f32;
    let line_height = font.line_metrics(style.text).line_height;
    let lines = font.wrap(style.text, text, width);
    let total = line_height * lines.len() as f32;
    let mut baseline =
        f32::from(area.y) + (f32::from(area.height) - total) / 2.0 + line_height * 0.8;
    for range in lines {
        let line = &text[range];
        let x = f32::from(area.x) + (f32::from(area.width) - font.measure(style.text, line)) / 2.0;
        font.draw(canvas, style.text, x, baseline as i32, line, Color::BLACK);
        baseline += line_height;
    }
}

/// `text` shortened with an ellipsis to fit `width` pixels.
pub fn fit(font: &FontHandle<'_>, size: f32, text: &str, width: f32) -> String {
    if font.measure(size, text) <= width {
        return text.to_string();
    }
    let ellipsis = font.measure(size, "…");
    let mut result = String::new();
    let mut used = 0.0;
    for c in text.chars() {
        let w = font.advance(size, c);
        if used + w + ellipsis > width {
            break;
        }
        used += w;
        result.push(c);
    }
    result.push('…');
    result
}

/// Fills a rectangle.
pub fn fill(canvas: &mut Canvas<'_>, area: Rect, color: Color) {
    canvas.fill_rect(
        i32::from(area.x),
        i32::from(area.y),
        u32::from(area.width),
        u32::from(area.height),
        color,
    );
}

/// What changed after moving a list's focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListChange {
    None,
    /// The focus moved within the visible page.
    Focus,
    /// The visible page changed.
    Page,
}

/// A paged, focusable list — the basic building block of menus, the
/// library and settings. Works with buttons (focus) and touch (hit tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListView {
    /// Where the rows are drawn.
    pub area: Rect,
    pub row: u32,
    pub focus: usize,
}

impl ListView {
    pub fn new(area: Rect, row: u32) -> Self {
        Self {
            area,
            row: row.max(1),
            focus: 0,
        }
    }

    /// Rows per page.
    pub fn rows(&self) -> usize {
        (u32::from(self.area.height) / self.row).max(1) as usize
    }

    /// The page containing the focus (0-based).
    pub fn page(&self) -> usize {
        self.focus / self.rows()
    }

    pub fn pages(&self, len: usize) -> usize {
        len.div_ceil(self.rows()).max(1)
    }

    /// Moves the focus by `delta` rows, clamped to `0..len`.
    pub fn move_focus(&mut self, delta: isize, len: usize) -> ListChange {
        if len == 0 {
            return ListChange::None;
        }
        let focus = (self.focus as isize + delta).clamp(0, len as isize - 1) as usize;
        if focus == self.focus {
            return ListChange::None;
        }
        let page = self.page();
        self.focus = focus;
        if self.page() == page {
            ListChange::Focus
        } else {
            ListChange::Page
        }
    }

    /// The rectangle of visible row `slot` (0 = top of the page).
    pub fn row_rect(&self, slot: usize) -> Rect {
        Rect::new(
            self.area.x,
            self.area.y + (slot as u32 * self.row) as u16,
            self.area.width,
            self.row as u16,
        )
    }

    /// The index of the row at `point`, if any.
    pub fn hit(&self, point: Point, len: usize) -> Option<usize> {
        if !self.area.contains(point) {
            return None;
        }
        let slot = ((point.y - i32::from(self.area.y)) as u32 / self.row) as usize;
        let index = self.page() * self.rows() + slot;
        (slot < self.rows() && index < len).then_some(index)
    }

    /// Draws the visible rows: the focused row inverted, the others with a
    /// separator. `draw_row(canvas, index, rect, focused)` draws a row's
    /// content in black, or white when focused.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        style: &UiStyle,
        len: usize,
        mut draw_row: impl FnMut(&mut Canvas<'_>, usize, Rect, bool),
    ) {
        let first = self.page() * self.rows();
        for (slot, index) in (first..len).take(self.rows()).enumerate() {
            let rect = self.row_rect(slot);
            let focused = index == self.focus;
            if focused {
                fill(canvas, rect, Color::BLACK);
            } else {
                let separator = Rect::new(
                    rect.x + style.margin as u16,
                    rect.y + rect.height - style.line as u16,
                    rect.width.saturating_sub(2 * style.margin as u16),
                    style.line as u16,
                );
                fill(canvas, separator, Color::gray(170));
            }
            draw_row(canvas, index, rect, focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_pages_focus_and_hits() {
        let mut list = ListView::new(Rect::new(0, 100, 200, 300), 100);
        assert_eq!(list.rows(), 3);
        assert_eq!(list.pages(7), 3);
        assert_eq!(list.move_focus(1, 7), ListChange::Focus);
        assert_eq!(list.move_focus(2, 7), ListChange::Page);
        assert_eq!(list.page(), 1);
        assert_eq!(list.move_focus(100, 7), ListChange::Page);
        assert_eq!(list.focus, 6);
        assert_eq!(list.move_focus(1, 7), ListChange::None);
        // Page 2 shows only index 6.
        assert_eq!(list.hit(Point::new(10, 150), 7), Some(6));
        assert_eq!(list.hit(Point::new(10, 250), 7), None);
        assert_eq!(list.hit(Point::new(10, 50), 7), None);
        assert_eq!(list.move_focus(1, 0), ListChange::None);
    }
}

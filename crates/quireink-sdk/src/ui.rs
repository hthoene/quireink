//! Building blocks for plugin user interfaces.
//!
//! All sizes derive from the device's density and the `core.ui-size`
//! setting, so every plugin's UI looks consistent and scales together when
//! the user chooses larger text.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_document::ImageRef;
use quireink_graphics::Canvas;
use quireink_image::GrayFile;
use quireink_platform::{Color, Point, Rect, Storage};

use crate::builtin::{actions, fonts, settings};
use crate::env::Env;
use crate::fonts::FontHandle;
use crate::settings::{SettingKind, SettingSpec, SettingValue};

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

    /// Height of a row with a note: the title line, then up to
    /// [`NOTE_LINES`] lines of small text (see [`draw_noted_row`]).
    pub fn noted_row(&self) -> u32 {
        let notes = (NOTE_LINES as f32 * self.small * NOTE_LINE_HEIGHT) as u32;
        self.title_band() + notes + self.gap * 3 / 2
    }

    /// Height of the title line of a noted row.
    fn title_band(&self) -> u32 {
        self.row * 3 / 4
    }
}

/// Lines of a row's note, e.g. a setting's description.
pub const NOTE_LINES: usize = 2;

/// Line height of notes, relative to the small text size.
const NOTE_LINE_HEIGHT: f32 = 1.3;

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

/// A screen's header bar for touch screens too: a back arrow on the left
/// and, for lists with several pages, arrows around the page number on the
/// right — so every screen can be left and paged without buttons or hidden
/// gestures. Without touch the arrows are left out (the buttons do that).
///
/// Screens draw it with [`Header::draw`] and pass taps to [`Header::tap`],
/// which tells the action to run (`core.back`, `core.prev-page`,
/// `core.next-page`); the screen handles paging like the page buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub area: Rect,
    /// Shows the back arrow.
    back: bool,
    /// Shows arrows for paging.
    arrows: bool,
}

/// The parts of a header, from [`Header::layout`].
struct HeaderParts {
    back: Option<Rect>,
    /// Previous page, page label, next page.
    pager: Option<(Rect, Rect, Rect)>,
    title: Rect,
}

impl Header {
    /// A header at the top of the screen; `back` is `false` for root
    /// screens such as the home screen.
    pub fn new(env: &Env<'_>, style: &UiStyle, back: bool) -> Self {
        let touch = env.device.input.touch;
        Self {
            area: Rect::new(0, 0, env.device.display.width, style.header as u16),
            back: back && touch,
            arrows: touch,
        }
    }

    fn page_label(page: (usize, usize)) -> String {
        format!("{} / {}", page.0 + 1, page.1)
    }

    fn layout(&self, env: &Env<'_>, style: &UiStyle, page: Option<(usize, usize)>) -> HeaderParts {
        let a = self.area;
        let square = a.height;
        let back = self.back.then(|| Rect::new(a.x, a.y, square, a.height));
        let mut left = a.x
            + if self.back {
                square
            } else {
                style.margin as u16
            };
        let mut right = a.x + a.width;
        let pager = match (page, style.font(env)) {
            (Some(page), Some(font)) if page.1 > 1 => {
                let label = font.measure(style.small, &Self::page_label(page)) as u16;
                let arrow = if self.arrows { square * 4 / 5 } else { 0 };
                let margin = if self.arrows { 0 } else { style.margin as u16 };
                let next = Rect::new(right - arrow - margin, a.y, arrow, a.height);
                let label = Rect::new(next.x - label, a.y, label, a.height);
                let prev = Rect::new(label.x - arrow, a.y, arrow, a.height);
                right = prev.x;
                Some((prev, label, next))
            }
            _ => None,
        };
        if self.back {
            left = left.min(right);
        }
        HeaderParts {
            back,
            pager,
            title: Rect::new(left, a.y, right.saturating_sub(left), a.height),
        }
    }

    /// Draws the header: the title, an optional note right of it (e.g. a
    /// sort order) and the page number of a paged list (`page` is the
    /// 0-based page and the number of pages).
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        env: &Env<'_>,
        style: &UiStyle,
        title: &str,
        note: Option<&str>,
        page: Option<(usize, usize)>,
    ) {
        let parts = self.layout(env, style, page);
        fill(canvas, self.area, Color::BLACK);
        let Some(font) = style.font(env) else {
            return;
        };
        let arrow = |canvas: &mut Canvas<'_>, area: Rect, left: bool, color: Color| {
            let (x, y) = center(area);
            draw_chevron(canvas, style, x, y, left, color);
        };
        if let Some(back) = parts.back {
            arrow(canvas, back, true, Color::WHITE);
        }
        if let (Some((prev, label, next)), Some(page)) = (parts.pager, page) {
            if self.arrows {
                // At the first or last page, the arrow is gray.
                let color = |end: bool| if end { Color::gray(110) } else { Color::WHITE };
                arrow(canvas, prev, true, color(page.0 == 0));
                arrow(canvas, next, false, color(page.0 + 1 >= page.1));
            }
            let text = Self::page_label(page);
            draw_label(
                canvas,
                &font,
                style.small,
                f32::from(label.x),
                label,
                &text,
                Color::WHITE,
            );
        }
        let t = parts.title;
        let margin = style.margin as f32;
        let mut right = f32::from(t.x) + f32::from(t.width) - margin;
        let mut canvas = canvas.clipped(t);
        if let Some(note) = note {
            right -= font.measure(style.small, note);
            draw_label(
                &mut canvas,
                &font,
                style.small,
                right,
                t,
                note,
                Color::WHITE,
            );
            right -= margin;
        }
        let title = fit(&font, style.title, title, right - f32::from(t.x));
        draw_label(
            &mut canvas,
            &font,
            style.title,
            f32::from(t.x),
            t,
            &title,
            Color::WHITE,
        );
    }

    /// The action a tap at `point` stands for: back, or the previous or
    /// next page. `None` outside the header or on its title.
    pub fn tap(
        &self,
        env: &Env<'_>,
        style: &UiStyle,
        point: Point,
        page: Option<(usize, usize)>,
    ) -> Option<crate::Id> {
        if !self.area.contains(point) {
            return None;
        }
        let parts = self.layout(env, style, page);
        if parts.back.is_some_and(|r| r.contains(point)) {
            return Some(actions::BACK);
        }
        let (prev, label, next) = parts.pager.filter(|_| self.arrows)?;
        if prev.contains(point) || (label.contains(point) && point.x < label_mid(label)) {
            Some(actions::PREV_PAGE)
        } else if next.contains(point) || label.contains(point) {
            Some(actions::NEXT_PAGE)
        } else {
            None
        }
    }

    /// Whether `point` is on the header's title (e.g. to open a menu).
    pub fn on_title(
        &self,
        env: &Env<'_>,
        style: &UiStyle,
        point: Point,
        page: Option<(usize, usize)>,
    ) -> bool {
        self.layout(env, style, page).title.contains(point)
    }
}

/// A bar under a list without a header (a menu, a popup) that shows the
/// page and, on touch screens, has arrows to turn pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pager {
    pub area: Rect,
    arrows: bool,
}

impl Pager {
    pub fn new(env: &Env<'_>, area: Rect) -> Self {
        Self {
            area,
            arrows: env.device.input.touch,
        }
    }

    /// The height of a pager.
    pub fn height(style: &UiStyle) -> u16 {
        (style.row * 3 / 4) as u16
    }

    /// Draws page `page.0` (0-based) of `page.1`.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        env: &Env<'_>,
        style: &UiStyle,
        page: (usize, usize),
    ) {
        let a = self.area;
        fill(canvas, a, Color::WHITE);
        canvas.fill_rect(
            i32::from(a.x) + style.margin as i32,
            i32::from(a.y),
            u32::from(a.width).saturating_sub(style.margin * 2),
            style.line.max(1),
            Color::gray(150),
        );
        let Some(font) = style.font(env) else {
            return;
        };
        let label = Header::page_label(page);
        let x = f32::from(a.x) + (f32::from(a.width) - font.measure(style.small, &label)) / 2.0;
        draw_label(canvas, &font, style.small, x, a, &label, Color::BLACK);
        if self.arrows {
            let side = i32::from(a.width) / 4;
            let (_, y) = center(a);
            let dim = |end: bool| if end { Color::gray(170) } else { Color::BLACK };
            let x = i32::from(a.x);
            draw_chevron(canvas, style, x + side, y, true, dim(page.0 == 0));
            draw_chevron(
                canvas,
                style,
                x + side * 3,
                y,
                false,
                dim(page.0 + 1 >= page.1),
            );
        }
    }

    /// `core.prev-page` or `core.next-page` for a tap on the left or right
    /// half; `None` outside.
    pub fn tap(&self, point: Point) -> Option<crate::Id> {
        if !self.arrows || !self.area.contains(point) {
            return None;
        }
        Some(if point.x < label_mid(self.area) {
            actions::PREV_PAGE
        } else {
            actions::NEXT_PAGE
        })
    }
}

fn center(area: Rect) -> (i32, i32) {
    (
        i32::from(area.x) + i32::from(area.width) / 2,
        i32::from(area.y) + i32::from(area.height) / 2,
    )
}

/// An arrow head pointing left or right, centred at `(x, y)` and as tall
/// as a capital letter of titles. Drawn rather than taken from a font, so
/// it is centred exactly and needs no special glyph.
pub fn draw_chevron(
    canvas: &mut Canvas<'_>,
    style: &UiStyle,
    x: i32,
    y: i32,
    left: bool,
    color: Color,
) {
    let half = (style.title * 0.38) as i32;
    let width = half * 4 / 5;
    let thickness = (style.line * 3).max(3);
    for dy in -half..=half {
        let dx = width * dy.abs() / half.max(1);
        let tip = if left {
            x - width / 2 + dx
        } else {
            x + width / 2 - dx
        };
        let start = if left {
            tip
        } else {
            tip - thickness as i32 + 1
        };
        canvas.fill_rect(start, y + dy, thickness, 1, color);
    }
}

fn label_mid(label: Rect) -> i32 {
    i32::from(label.x) + i32::from(label.width) / 2
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
    /// Whether the focused row is highlighted; off while the focus is on
    /// another element of the screen.
    pub active: bool,
}

impl ListView {
    pub fn new(area: Rect, row: u32) -> Self {
        Self {
            area,
            row: row.max(1),
            focus: 0,
            active: true,
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

    /// The page and the number of pages, e.g. for [`Header::draw`].
    pub fn paging(&self, len: usize) -> Option<(usize, usize)> {
        Some((self.page(), self.pages(len)))
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
            let focused = self.active && index == self.focus;
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

// ---- Settings as rows --------------------------------------------------

/// The part of a noted row (see [`UiStyle::noted_row`]) that holds the
/// title and the value, or a stepper; the note goes below it.
pub fn title_area(style: &UiStyle, rect: Rect) -> Rect {
    let band = (style.title_band() as u16).min(rect.height);
    Rect::new(rect.x, rect.y + style.gap as u16 / 2, rect.width, band)
}

/// Draws a row with `title` and `value` on its first line and `note` in
/// small text below, wrapped to [`NOTE_LINES`] lines and shortened with
/// "…" if longer. Rows are always fully readable before they are tapped,
/// so a description never needs a tap that would change a setting.
/// Without a note, the row looks like [`draw_value_row`].
#[allow(clippy::too_many_arguments)]
pub fn draw_noted_row(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    rect: Rect,
    focused: bool,
    title: &str,
    value: &str,
    note: &str,
) {
    if note.is_empty() {
        return draw_value_row(canvas, env, style, rect, focused, title, value);
    }
    let band = title_area(style, rect);
    draw_value_row(canvas, env, style, band, focused, title, value);
    draw_note(canvas, env, style, rect, focused, note);
}

/// Draws the note of a noted row below its [`title_area`].
pub fn draw_note(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    rect: Rect,
    focused: bool,
    note: &str,
) {
    let Some(font) = style.font(env) else {
        return;
    };
    let band = title_area(style, rect);
    let margin = style.margin as f32;
    let width = f32::from(rect.width) - 2.0 * margin;
    let color = if focused {
        Color::gray(215)
    } else {
        Color::gray(80)
    };
    let line = style.small * NOTE_LINE_HEIGHT;
    let ascent = font.line_metrics(style.small).ascent;
    let top = f32::from(band.y + band.height);
    let lines = font.wrap(style.small, note, width);
    for (n, range) in lines.iter().take(NOTE_LINES).enumerate() {
        let last = n + 1 == NOTE_LINES && lines.len() > NOTE_LINES;
        let text = if last {
            fit(&font, style.small, note[range.start..].trim_end(), width)
        } else {
            note[range.clone()].to_string()
        };
        let baseline = top + line * n as f32 + ascent;
        font.draw(
            canvas,
            style.small,
            f32::from(rect.x) + margin,
            baseline as i32,
            &text,
            color,
        );
    }
}

/// Draws a row: `title` on the left, `value` right-aligned.
#[allow(clippy::too_many_arguments)]
pub fn draw_value_row(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    rect: Rect,
    focused: bool,
    title: &str,
    value: &str,
) {
    let Some(font) = style.font(env) else {
        return;
    };
    let color = if focused { Color::WHITE } else { Color::BLACK };
    let margin = style.margin as f32;
    let value_width = font.measure(style.text, value);
    let value_x = f32::from(rect.x) + f32::from(rect.width) - margin - value_width;
    draw_label(canvas, &font, style.text, value_x, rect, value, color);
    let title_x = f32::from(rect.x) + margin;
    let title = fit(&font, style.text, title, value_x - title_x - margin * 2.0);
    draw_label(canvas, &font, style.text, title_x, rect, &title, color);
}

/// The − and + buttons of a focused number row, with room for `value`
/// between them.
pub fn stepper(env: &Env<'_>, style: &UiStyle, rect: Rect, value: &str) -> Option<(Rect, Rect)> {
    let font = style.font(env)?;
    let size = (u32::from(rect.height) * 7 / 10) as u16;
    let y = rect.y + (rect.height - size) / 2;
    let margin = style.margin as u16;
    let gap = style.gap as u16;
    let right = rect.x + rect.width - margin;
    let plus = Rect::new(right.saturating_sub(size), y, size, size);
    let value_width = font.measure(style.text, value) as u16;
    let minus_x = plus.x.saturating_sub(2 * gap + value_width + size);
    Some((Rect::new(minus_x, y, size, size), plus))
}

/// A focused number: title, then `[−] value [+]`. While editing with
/// buttons the boxes are filled.
pub fn draw_stepper(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    rect: Rect,
    title: &str,
    value: &str,
    editing: bool,
) {
    let (Some(font), Some((minus, plus))) = (style.font(env), stepper(env, style, rect, value))
    else {
        return;
    };
    let margin = style.margin as f32;
    let title = fit(&font, style.text, title, f32::from(minus.x) - margin * 2.0);
    draw_label(
        canvas,
        &font,
        style.text,
        margin,
        rect,
        &title,
        Color::WHITE,
    );
    let value_x = f32::from(minus.x + minus.width + style.gap as u16);
    draw_label(
        canvas,
        &font,
        style.text,
        value_x,
        rect,
        value,
        Color::WHITE,
    );
    for (button, glyph) in [(minus, "−"), (plus, "+")] {
        let (x, y, w, h) = (
            i32::from(button.x),
            i32::from(button.y),
            u32::from(button.width),
            u32::from(button.height),
        );
        let glyph_color = if editing {
            canvas.fill_rect(x, y, w, h, Color::WHITE);
            Color::BLACK
        } else {
            canvas.stroke_rect(x, y, w, h, style.line.max(2), Color::WHITE);
            Color::WHITE
        };
        let glyph_width = font.measure(style.text, glyph);
        let glyph_x = f32::from(button.x) + (f32::from(button.width) - glyph_width) / 2.0;
        draw_label(canvas, &font, style.text, glyph_x, rect, glyph, glyph_color);
    }
}

/// The value of a setting for display, with its unit.
pub fn setting_value(env: &Env<'_>, spec: &SettingSpec) -> String {
    match env.settings.get(spec.id.as_str()) {
        Some(SettingValue::Bool(true)) => env.i18n.tr("settings.on").to_string(),
        Some(SettingValue::Bool(false)) => env.i18n.tr("settings.off").to_string(),
        Some(SettingValue::Int(v)) => match &spec.unit {
            Some(unit) => env.i18n.plural(unit, *v, &[]),
            None => format!("{v}"),
        },
        Some(SettingValue::Text(v)) => option_name(env, spec, v),
        None => String::new(),
    }
}

/// The display name of a choice or language value.
pub fn option_name(env: &Env<'_>, spec: &SettingSpec, value: &str) -> String {
    match spec.kind {
        SettingKind::Language => env
            .i18n
            .languages()
            .into_iter()
            .find(|(tag, _)| *tag == value)
            .map_or(value.to_string(), |(_, name)| name.to_string()),
        // Choices are ids; their translation is their display name.
        SettingKind::Choice(_) => env.i18n.tr(value).to_string(),
        SettingKind::List => {
            let names: Vec<&str> = crate::status::item_ids(value)
                .map(|id| env.i18n.tr(env.status_items.title(id).unwrap_or(id)))
                .collect();
            if names.is_empty() {
                env.i18n.tr("core.list.none").to_string()
            } else {
                names.join(", ")
            }
        }
        _ => value.to_string(),
    }
}

/// The value after one step of the setting: a switch toggles, a number
/// moves by `step` within its range, a choice or language moves to the
/// next (or previous) option. `None` for text.
pub fn next_value(env: &Env<'_>, spec: &SettingSpec, step: i64) -> Option<SettingValue> {
    let current = env.settings.get(spec.id.as_str())?;
    let cycle = |options: &[&str], value: &str| -> SettingValue {
        let len = options.len().max(1) as i64;
        let index = options.iter().position(|o| *o == value).unwrap_or(0) as i64;
        let next = options
            .get((index + step).rem_euclid(len) as usize)
            .copied()
            .unwrap_or_default();
        SettingValue::Text(Cow::Owned(next.to_string()))
    };
    Some(match (&spec.kind, current) {
        (SettingKind::Bool, SettingValue::Bool(v)) => SettingValue::Bool(!v),
        (SettingKind::Int { min, max }, SettingValue::Int(v)) => {
            SettingValue::Int((v + step).clamp(*min, *max))
        }
        (SettingKind::Choice(options), SettingValue::Text(v)) => {
            let options: Vec<&str> = options.iter().map(|o| o.as_ref()).collect();
            cycle(&options, v)
        }
        (SettingKind::Language, SettingValue::Text(v)) => {
            let tags: Vec<&str> = env.i18n.languages().into_iter().map(|(t, _)| t).collect();
            cycle(&tags, v)
        }
        _ => return None,
    })
}

// ---- Images ------------------------------------------------------------

/// Draws a decoded gray image (e.g. a book's cover or an illustration)
/// scaled into `area`: each screen row reads one source row from storage,
/// columns are averaged when the image is reduced, and only rows inside
/// the canvas clip are read. Grays are dithered on two-level panels.
pub fn draw_image(canvas: &mut Canvas<'_>, storage: &dyn Storage, image: &ImageRef, area: Rect) {
    let file = GrayFile {
        path: image.path.clone(),
        width: image.width,
        height: image.height,
    };
    let (left, top) = (i32::from(area.x), i32::from(area.y));
    let (w, h) = (i32::from(area.width), i32::from(area.height));
    if w <= 0 || h <= 0 || file.width == 0 || file.height == 0 {
        return;
    }
    let clip = canvas.clip();
    let first = top.max(i32::from(clip.y));
    let last = (top + h).min(clip.bottom() as i32);
    if first >= last {
        return;
    }
    let mut source = alloc::vec![0u8; file.width as usize];
    let mut row = alloc::vec![0u8; w as usize];
    let mut loaded = None;
    for y in first..last {
        let sy = ((y - top) as u64 * u64::from(file.height) / h as u64) as u32;
        if loaded != Some(sy) {
            if !file.read_row(storage, sy, &mut source) {
                return;
            }
            loaded = Some(sy);
            let sw = file.width as usize;
            for (x, out) in row.iter_mut().enumerate() {
                let from = x * sw / w as usize;
                let to = ((x + 1) * sw / w as usize).max(from + 1).min(sw);
                let sum: u32 = source[from..to].iter().map(|&v| u32::from(v)).sum();
                *out = (sum / (to - from) as u32) as u8;
            }
        }
        canvas.draw_gray_row(left, y, &row);
    }
}

/// The largest size of `image` that fits `width` × `height`, keeping its
/// proportions.
pub fn fit_image(image: &ImageRef, width: u16, height: u16) -> (u16, u16) {
    if image.width == 0 || image.height == 0 {
        return (0, 0);
    }
    let by_width = u64::from(width) * u64::from(image.height) / u64::from(image.width);
    if by_width <= u64::from(height) {
        (width, by_width as u16)
    } else {
        let w = u64::from(height) * u64::from(image.width) / u64::from(image.height);
        (w as u16, height)
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

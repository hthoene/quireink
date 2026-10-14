//! Status items: small pieces of information shown in a status line, such
//! as the time, the battery or the page.
//!
//! Any plugin adds items with
//! [`Registrar::add_status_item`](crate::Registrar::add_status_item);
//! screens with a status line let the user choose which items it shows and
//! where (the reader does this for its lines above and below the text).
//! An item computes its value from the environment and from the context the
//! screen passes, e.g. the reader's `title`, `chapter`, `page` and
//! `progress` (see [`context`]).

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use quireink_graphics::Canvas;
use quireink_platform::{Color, Rect};

use crate::env::Env;
use crate::fonts::FontHandle;
use crate::id::Id;
use crate::params::Params;

/// Parameters screens pass to status items. Items ignore what they do not
/// need; a screen passes what it knows.
pub mod context {
    /// The open book's path, title and reading progress in percent are
    /// [`params::PATH`](crate::builtin::params::PATH),
    /// [`params::TITLE`](crate::builtin::params::TITLE) and
    /// [`params::PROGRESS`](crate::builtin::params::PROGRESS).
    pub const CHAPTER: &str = "chapter";
    /// The page as shown, e.g. `12 / 340` (or a percentage while pages are
    /// being counted).
    pub const PAGE: &str = "page";
}

/// What a status item shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusValue {
    Text(String),
    /// A gauge with a percentage, drawn as a battery.
    Level {
        percent: u8,
        charging: bool,
    },
}

/// Computes an item's value; `None` hides it (e.g. no battery).
pub type StatusFn = Box<dyn Fn(&Env<'_>, &Params) -> Option<StatusValue>>;

/// A status item contribution.
pub struct StatusItemSpec {
    pub id: Id,
    /// Translation key of its name in settings, e.g. "Battery".
    pub title: Cow<'static, str>,
    pub value: StatusFn,
}

impl fmt::Debug for StatusItemSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatusItemSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

/// The registered status items.
#[derive(Debug, Default)]
pub struct StatusItems {
    items: Vec<StatusItemSpec>,
}

impl StatusItems {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, item: StatusItemSpec) {
        self.items.push(item);
    }

    pub fn shrink_to_fit(&mut self) {
        self.items.shrink_to_fit();
    }

    /// Every item: its id and the translation key of its name.
    pub fn iter(&self) -> impl Iterator<Item = (&Id, &str)> {
        self.items.iter().map(|i| (&i.id, i.title.as_ref()))
    }

    /// The translation key of the name of item `id`.
    pub fn title(&self, id: &str) -> Option<&str> {
        self.items
            .iter()
            .find(|i| i.id.as_str() == id)
            .map(|i| i.title.as_ref())
    }

    /// The value of item `id` now.
    pub fn value(&self, id: &str, env: &Env<'_>, context: &Params) -> Option<StatusValue> {
        let item = self.items.iter().find(|i| i.id.as_str() == id)?;
        (item.value)(env, context)
    }
}

/// The items of a list as stored in a setting: ids separated by commas.
pub fn item_ids(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(str::trim).filter(|id| !id.is_empty())
}

/// A status line: the items listed (comma-separated ids) for its left,
/// centre and right, drawn in `area` with text of `size` in `color`.
#[derive(Debug, Clone, Copy)]
pub struct StatusLine<'a> {
    pub area: Rect,
    pub lists: [&'a str; 3],
    pub size: f32,
    pub color: Color,
}

impl StatusLine<'_> {
    /// Whether no item is listed.
    pub fn is_empty(&self) -> bool {
        self.lists
            .iter()
            .all(|list| item_ids(list).next().is_none())
    }

    /// Draws the items' current values. Long texts (a chapter's title) are
    /// shortened with "…": the right part takes at most half of the line
    /// while others show, the centre what is left beside it, the left part
    /// the rest.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        env: &Env<'_>,
        font: &FontHandle<'_>,
        context: &Params,
    ) {
        let (area, size) = (self.area, self.size);
        let gap = size;
        let values = |list: &str| -> Vec<StatusValue> {
            item_ids(list)
                .filter_map(|id| env.status_items.value(id, env, context))
                .collect()
        };
        let [left, center, right] = self.lists.map(values);
        let width = |values: &[StatusValue]| -> f32 {
            values
                .iter()
                .map(|v| value_width(font, size, v))
                .sum::<f32>()
                + gap * values.len().saturating_sub(1) as f32
        };
        let x0 = f32::from(area.x);
        let x1 = x0 + f32::from(area.width);
        let line = f32::from(area.width);
        let others = !left.is_empty() || !center.is_empty();
        let right_x = x1 - width(&right).min(if others { line / 2.0 } else { line });
        // Room for the centre: beside the right part, leaving the left a
        // quarter of the line if it shows anything.
        let left_room = if left.is_empty() { 0.0 } else { line / 4.0 };
        let center_room = (right_x - gap - x0 - left_room).max(0.0);
        let center_w = width(&center).min(center_room);
        // Centred, unless that runs into the right part.
        let center_x = (x0 + (line - center_w) / 2.0)
            .min(right_x - gap - center_w)
            .max(x0 + left_room);
        let left_end = if center.is_empty() { right_x } else { center_x } - gap;
        let mut draw = |values: &[StatusValue], x: f32, end: f32| {
            draw_values(canvas, font, area, values, (x, end), size, self.color);
        };
        draw(&center, center_x, right_x - gap);
        draw(&right, right_x, x1);
        draw(&left, x0, left_end);
    }
}

fn value_width(font: &FontHandle<'_>, size: f32, value: &StatusValue) -> f32 {
    match value {
        StatusValue::Text(text) => font.measure(size, text),
        StatusValue::Level { percent, .. } => {
            battery_width(size) + size * 0.3 + font.measure(size, &level_text(*percent))
        }
    }
}

fn level_text(percent: u8) -> String {
    alloc::format!("{percent} %")
}

fn battery_width(size: f32) -> f32 {
    size * 1.3
}

fn draw_values(
    canvas: &mut Canvas<'_>,
    font: &FontHandle<'_>,
    area: Rect,
    values: &[StatusValue],
    (mut x, end): (f32, f32),
    size: f32,
    color: Color,
) {
    let gap = size;
    let metrics = font.line_metrics(size);
    let baseline = i32::from(area.y)
        + ((f32::from(area.height) + metrics.ascent - metrics.descent) / 2.0) as i32;
    let widths: Vec<f32> = values.iter().map(|v| value_width(font, size, v)).collect();
    let texts: Vec<bool> = values
        .iter()
        .map(|v| matches!(v, StatusValue::Text(_)))
        .collect();
    let shrunk = shrink(&widths, &texts, gap, end - x);
    let budget = |i: usize| match shrunk {
        Some((at, width)) if at == i => width,
        _ => f32::MAX,
    };
    for (i, value) in values.iter().enumerate() {
        if x >= end {
            break;
        }
        match value {
            StatusValue::Text(text) => {
                // A pixel of slack for rounding: the parts were measured to fit.
                let room = (end - x + 1.0).min(budget(i));
                let text = crate::ui::fit(font, size, text, room);
                x += font.draw(canvas, size, x, baseline, &text, color);
            }
            StatusValue::Level { percent, charging } => {
                let height = (metrics.ascent * 0.75) as i32;
                let top = baseline - height;
                let battery = Rect::new(
                    x as u16,
                    top.max(0) as u16,
                    battery_width(size) as u16,
                    height as u16,
                );
                draw_battery(canvas, battery, size, *percent, *charging, color);
                x += battery_width(size) + size * 0.3;
                x += font.draw(canvas, size, x, baseline, &level_text(*percent), color);
            }
        }
        x += gap;
    }
}

/// When values of `widths` (with `gap`s between them) do not fit `room`:
/// the longest text (`texts`) and the width it may take, so short ones
/// (the page, the time) stay whole. A pixel of slack for rounding: parts
/// are placed by their own width.
fn shrink(widths: &[f32], texts: &[bool], gap: f32, room: f32) -> Option<(usize, f32)> {
    let total = widths.iter().sum::<f32>() + gap * widths.len().saturating_sub(1) as f32;
    if total <= room + 1.0 {
        return None;
    }
    let longest = (0..widths.len())
        .filter(|&i| texts[i])
        .max_by(|&a, &b| widths[a].total_cmp(&widths[b]))?;
    Some((longest, (widths[longest] - (total - room)).max(0.0)))
}

/// A battery outline filled to `percent`; a plus when charging.
fn draw_battery(
    canvas: &mut Canvas<'_>,
    area: Rect,
    size: f32,
    percent: u8,
    charging: bool,
    color: Color,
) {
    let (x, y, height) = (i32::from(area.x), i32::from(area.y), i32::from(area.height));
    let line = ((size / 10.0) as u32).max(1);
    let nub = ((size / 8.0) as i32).max(2);
    let width = i32::from(area.width) - nub;
    canvas.stroke_rect(x, y, width as u32, height as u32, line, color);
    canvas.fill_rect(
        x + width,
        y + height / 4,
        nub as u32,
        (height / 2) as u32,
        color,
    );
    let inner = width - 2 * (line as i32 + 1);
    let fill = inner * i32::from(percent.min(100)) / 100;
    let inset = line as i32 + 1;
    canvas.fill_rect(
        x + inset,
        y + inset,
        fill.max(0) as u32,
        (height - 2 * inset).max(0) as u32,
        color,
    );
    if charging {
        // A plus right of the battery, in the gap before the percentage.
        let s = (height / 2).max(3);
        let cx = x + width / 2;
        let cy = y + height / 2;
        let bar = (line as i32).max(1);
        let ink = if percent > 50 { Color::WHITE } else { color };
        canvas.fill_rect(cx - s / 2, cy - bar / 2, s as u32, bar as u32, ink);
        canvas.fill_rect(cx - bar / 2, cy - s / 2, bar as u32, s as u32, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lines_too_long_give_way() {
        // Battery and time placed by their own width: rounding noise is
        // no reason to cut the time.
        let widths = [60.0, 70.1];
        let room = 70.1 + 60.0 + 20.0 - 0.000_01;
        assert_eq!(shrink(&widths, &[false, true], 20.0, room), None);
        // Too long: the longest text gives way, the page stays whole.
        let widths = [300.0, 80.0];
        assert_eq!(
            shrink(&widths, &[true, true], 20.0, 250.0),
            Some((0, 150.0))
        );
        // Nothing to shorten without text.
        assert_eq!(shrink(&[100.0], &[false], 0.0, 50.0), None);
    }
}

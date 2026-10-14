//! Choosing text on the page and what can be done with it.
//!
//! Touch: a long press on a word selects it and shows a bar at the bottom
//! (look up, highlight, cancel); tapping more words extends the selection.
//! Buttons: *Select text* in the reader's menu, move to a word, confirm for
//! the bar, where *Extend* moves the end of the selection word by word.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::actions;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{Canvas, Color, Env, Id, Point, Rect};

use crate::SELECTION_MENU;
use crate::actions::{CANCEL, EXTEND, HIGHLIGHT, MORE};
use crate::screen::WordBox;

/// What the buttons do with the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Moving the cursor; the selection is one word.
    Choosing,
    /// Moving the end; the start stays.
    Extending,
    /// The bar is shown.
    Bar,
}

/// An item of the bar: an action listed in the menu `reader.selection`
/// (see [`crate::SELECTION_MENU`]), so plugins and users can add, remove
/// and reorder them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub id: Id,
    /// Translation key of its label.
    pub title: String,
}

/// Selected words, as indexes into the page's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Selection {
    pub anchor: usize,
    pub cursor: usize,
    pub mode: Mode,
    /// The bar item focused by buttons (none after a touch).
    pub focus: Option<usize>,
}

impl Selection {
    /// One word, chosen by touch: the bar is shown at once.
    pub fn touched(word: usize) -> Self {
        Self {
            anchor: word,
            cursor: word,
            mode: Mode::Bar,
            focus: None,
        }
    }

    /// One word, to be moved with the buttons.
    pub fn choosing(word: usize) -> Self {
        Self {
            anchor: word,
            cursor: word,
            mode: Mode::Choosing,
            focus: None,
        }
    }

    /// The first and last selected word.
    pub fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    /// Moves the cursor for a navigation action; `None` if `action` is
    /// another one.
    pub fn moved(&self, action: &Id, words: &[WordBox]) -> Option<usize> {
        let last = words.len().checked_sub(1)?;
        let a = action;
        Some(
            if [actions::LEFT, actions::PREVIOUS, actions::PREV_PAGE].contains(a) {
                self.cursor.saturating_sub(1)
            } else if [actions::RIGHT, actions::NEXT, actions::NEXT_PAGE].contains(a) {
                (self.cursor + 1).min(last)
            } else if *a == actions::UP {
                nearest_on_line(words, self.cursor, false)
            } else if *a == actions::DOWN {
                nearest_on_line(words, self.cursor, true)
            } else {
                return None;
            },
        )
    }
}

/// The bar's items: the selection menu's entries, as many as fit with
/// readable labels; if not all do, *More* before the last place opens the
/// whole menu as a list. *Cancel* always stays visible, at the end, since
/// tapping the page mostly hits words, which extends the selection.
/// *Highlight* reads *Remove highlight* on highlighted text. With buttons,
/// *Cancel* is left out (back cancels); on touch screens *Extend* is
/// (tapping words extends the selection).
pub(crate) fn items(env: &Env<'_>, highlighted: bool) -> Vec<Item> {
    let mut all: Vec<Item> = env
        .menus
        .items(SELECTION_MENU.as_str())
        .into_iter()
        .filter(|item| {
            let touch = env.device.input.touch;
            !(*item.id == EXTEND && touch || *item.id == CANCEL && !touch)
        })
        .map(|item| Item {
            id: item.id.clone(),
            title: if *item.id == HIGHLIGHT && highlighted {
                "reader.select.unhighlight".to_string()
            } else {
                item.title.to_string()
            },
        })
        .collect();
    let cancel: Vec<Item> = all
        .iter()
        .position(|item| item.id == CANCEL)
        .map(|at| all.remove(at))
        .into_iter()
        .collect();
    let style = UiStyle::new(env);
    let width = f32::from(bar_area(env, &style).width);
    let fits = |items: &[Item]| natural_widths(env, &style, items).iter().sum::<f32>() <= width;
    if fits(&[all.as_slice(), &cancel].concat()) {
        all.extend(cancel);
        return all;
    }
    let more = Item {
        id: MORE,
        title: "reader.select.more".to_string(),
    };
    let mut shown: Vec<Item> = Vec::new();
    for item in &all {
        let mut next = shown.clone();
        next.push(item.clone());
        next.push(more.clone());
        next.extend(cancel.iter().cloned());
        if !fits(&next) {
            break;
        }
        shown.push(item.clone());
    }
    shown.push(more);
    shown.extend(cancel);
    shown
}

/// Where the bar is: over the status line at the bottom.
pub(crate) fn bar_area(env: &Env<'_>, style: &UiStyle) -> Rect {
    let display = env.device.display;
    let height = style.row as u16;
    Rect::new(0, display.height - height, display.width, height)
}

/// How wide the items' labels are, with a gap around each.
fn natural_widths(env: &Env<'_>, style: &UiStyle, items: &[Item]) -> Vec<f32> {
    let font = style.font(env);
    items
        .iter()
        .map(|item| {
            let label = font
                .as_ref()
                .map_or(0.0, |f| f.measure(style.small, env.i18n.tr(&item.title)));
            label + style.gap as f32
        })
        .collect()
}

/// The rectangles of the bar's items: each as wide as its label needs,
/// the rest of the bar shared equally.
fn item_areas(env: &Env<'_>, style: &UiStyle, items: &[Item]) -> Vec<Rect> {
    let bar = bar_area(env, style);
    let natural = natural_widths(env, style, items);
    let spare =
        (f32::from(bar.width) - natural.iter().sum::<f32>()).max(0.0) / items.len().max(1) as f32;
    let mut left = f32::from(bar.x);
    let mut areas = Vec::with_capacity(items.len());
    for (index, width) in natural.iter().enumerate() {
        let right = if index + 1 == natural.len() {
            f32::from(bar.x) + f32::from(bar.width)
        } else {
            left + width + spare
        };
        let (x, end) = ((left + 0.5) as u16, (right + 0.5) as u16);
        areas.push(Rect::new(x, bar.y, end.saturating_sub(x), bar.height));
        left = right;
    }
    areas
}

pub(crate) fn draw_bar(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    items: &[Item],
    focus: Option<usize>,
) {
    let style = UiStyle::new(env);
    let bar = bar_area(env, &style);
    ui::fill(canvas, bar, Color::WHITE);
    let line = style.line.max(2);
    canvas.fill_rect(
        i32::from(bar.x),
        i32::from(bar.y),
        u32::from(bar.width),
        line,
        Color::BLACK,
    );
    let Some(font) = style.font(env) else {
        return;
    };
    let areas = item_areas(env, &style, items);
    for (index, (item, &area)) in items.iter().zip(&areas).enumerate() {
        let focused = focus == Some(index);
        if focused {
            ui::fill(canvas, area, Color::BLACK);
        } else if index > 0 {
            // A thin divider between items.
            let h = area.height / 2;
            canvas.fill_rect(
                i32::from(area.x),
                i32::from(area.y + h / 2),
                style.line.max(1),
                u32::from(h),
                Color::gray(150),
            );
        }
        let label = env.i18n.tr(&item.title);
        let label = ui::fit(&font, style.small, label, f32::from(area.width) - 8.0);
        let x =
            f32::from(area.x) + (f32::from(area.width) - font.measure(style.small, &label)) / 2.0;
        let color = if focused { Color::WHITE } else { Color::BLACK };
        ui::draw_label(canvas, &font, style.small, x, area, &label, color);
    }
}

/// The bar item at `point`, if any.
pub(crate) fn item_at(env: &Env<'_>, items: &[Item], point: Point) -> Option<Item> {
    let style = UiStyle::new(env);
    if !bar_area(env, &style).contains(point) {
        return None;
    }
    let areas = item_areas(env, &style, items);
    let index = areas.iter().position(|area| area.contains(point))?;
    items.get(index).cloned()
}

/// Boxes around runs of words on one line, e.g. to draw a selection or
/// highlight as one shape per line.
pub(crate) fn line_boxes(words: &[WordBox], mut picked: impl FnMut(usize) -> bool) -> Vec<Rect> {
    let mut boxes: Vec<Rect> = Vec::new();
    let mut previous: Option<usize> = None;
    for (index, word) in words.iter().enumerate() {
        if !picked(index) {
            previous = None;
            continue;
        }
        match boxes.last_mut() {
            Some(last) if previous == Some(index.wrapping_sub(1)) && last.y == word.area.y => {
                last.width = word.area.right().saturating_sub(u32::from(last.x)) as u16;
            }
            _ => boxes.push(word.area),
        }
        previous = Some(index);
    }
    boxes
}

/// The word on the line above (or below) word `index` nearest to it.
fn nearest_on_line(words: &[WordBox], index: usize, below: bool) -> usize {
    let current = &words[index].area;
    let center = |r: &Rect| i32::from(r.x) + i32::from(r.width) / 2;
    let line = words
        .iter()
        .map(|w| w.area.y)
        .filter(|&y| if below { y > current.y } else { y < current.y })
        .min_by_key(|&y| y.abs_diff(current.y));
    let Some(line) = line else {
        return index;
    };
    words
        .iter()
        .enumerate()
        .filter(|(_, w)| w.area.y == line)
        .min_by_key(|(_, w)| center(&w.area).abs_diff(center(current)))
        .map_or(index, |(i, _)| i)
}

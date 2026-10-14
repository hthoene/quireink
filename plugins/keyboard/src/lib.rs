//! The on-screen keyboard.
//!
//! Bound to the `core.keyboard` slot: any plugin that needs text (search,
//! a setting, a note) opens the slot and gets the text back as an event
//! (see [`slots::KEYBOARD`]). It is an overlay at the bottom of the screen
//! with a text field and the keys, used by tapping or with the buttons:
//! left and right move along a row, page forward and back (or up and
//! down) between rows, confirm presses the key, back cancels and a long
//! press on confirm finishes.
//!
//! The letter rows come from the translation key `keyboard.letters` (rows
//! separated by spaces), so a translation brings its own layout. Bind
//! another screen to `core.keyboard` for a different keyboard altogether.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params, Plugin,
    Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    Version,
};

pub const PLUGIN: Id = Id::new_static("keyboard");
pub const SCREEN: Id = Id::new_static("keyboard.screen");

/// Longest text the keyboard accepts, in characters.
const MAX_CHARS: usize = 200;

/// The rows of the symbol layer (the last one also gets *Delete*).
const SYMBOLS: [&str; 3] = ["1234567890", "@#&_-+()/%", "*\"':;!?"];

/// The keyboard plugin.
#[derive(Debug, Default)]
pub struct Keyboard;

impl Plugin for Keyboard {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Keyboard"),
            description: Cow::Borrowed("Enters text on the screen, by touch or with the buttons."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("keyboard.title"),
            factory: Box::new(|env, params| Box::new(KeyboardScreen::new(env, params))),
        })?;
        r.bind_slot(slots::KEYBOARD, SCREEN, Priority::DEFAULT)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Letters,
    Symbols,
    /// Only digits, for numbers (mode `number`).
    Digits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Char(char),
    Shift,
    Delete,
    Space,
    Layer(Layer),
    Done,
}

/// A key and its width in units (a letter is 2).
type Slot = (Key, u8);

/// The rows of a layer.
fn rows(env: &Env<'_>, layer: Layer) -> Vec<Vec<Slot>> {
    let chars = |row: &str| -> Vec<Slot> { row.chars().map(|c| (Key::Char(c), 2)).collect() };
    match layer {
        Layer::Letters | Layer::Symbols => {
            let mut rows: Vec<Vec<Slot>> = if layer == Layer::Letters {
                env.i18n
                    .tr("keyboard.letters")
                    .split_whitespace()
                    .map(chars)
                    .collect()
            } else {
                SYMBOLS.iter().map(|row| chars(row)).collect()
            };
            if let Some(last) = rows.last_mut() {
                if layer == Layer::Letters {
                    last.insert(0, (Key::Shift, 3));
                }
                last.push((Key::Delete, 3));
            }
            let other = if layer == Layer::Letters {
                Layer::Symbols
            } else {
                Layer::Letters
            };
            rows.push(alloc::vec![
                (Key::Layer(other), 3),
                (Key::Char(','), 2),
                (Key::Space, 10),
                (Key::Char('.'), 2),
                (Key::Done, 3),
            ]);
            rows
        }
        Layer::Digits => {
            let mut rows: Vec<Vec<Slot>> = ["123", "456", "789"].iter().map(|r| chars(r)).collect();
            rows.push(alloc::vec![
                (Key::Delete, 2),
                (Key::Char('0'), 2),
                (Key::Done, 2),
            ]);
            rows
        }
    }
}

struct KeyboardScreen {
    /// The opening parameters, sent back with the text.
    params: Params,
    title: String,
    text: String,
    layer: Layer,
    /// The next letter is a capital.
    shift: bool,
    rows: Vec<Vec<Slot>>,
    /// The key chosen with buttons, as `(row, index)`; none while tapping.
    focus: Option<(usize, usize)>,
    style: UiStyle,
    region: Rect,
    field: Rect,
    keys: Rect,
    key_height: u16,
}

impl KeyboardScreen {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let layer = if params.get(params::MODE) == Some("number") {
            Layer::Digits
        } else {
            Layer::Letters
        };
        let rows = rows(env, layer);
        let display = env.device.display;
        let key_height = style.row as u16;
        let field_height = (style.small * 1.5) as u16 + style.row as u16;
        let keys_height = key_height * rows.len().max(4) as u16;
        let gap = style.gap as u16 / 2;
        let height = (field_height + keys_height + gap * 3).min(display.height);
        let region = Rect::new(0, display.height - height, display.width, height);
        let field = Rect::new(0, region.y + gap, display.width, field_height);
        let keys = Rect::new(
            0,
            region.y + region.height - keys_height - gap,
            display.width,
            keys_height,
        );
        Self {
            title: params.get(params::TITLE).unwrap_or_default().to_string(),
            text: params.get(params::TEXT).unwrap_or_default().to_string(),
            params: params.clone(),
            layer,
            shift: false,
            rows,
            focus: (!env.device.input.touch).then_some((0, 0)),
            style,
            region,
            field,
            keys,
            key_height,
        }
    }

    /// Where key `index` of row `row` is.
    fn key_area(&self, row: usize, index: usize) -> Rect {
        let keys = &self.rows[row];
        let total: u32 = keys.iter().map(|(_, w)| u32::from(*w)).sum::<u32>().max(1);
        let width = u32::from(self.keys.width) - self.style.margin;
        let x0 = u32::from(self.keys.x) + self.style.margin / 2;
        let before: u32 = keys[..index].iter().map(|(_, w)| u32::from(*w)).sum();
        let left = x0 + width * before / total;
        let right = x0 + width * (before + u32::from(keys[index].1)) / total;
        let y = self.keys.y + self.key_height * row as u16;
        Rect::new(left as u16, y, (right - left) as u16, self.key_height)
    }

    fn key_at(&self, point: quireink_sdk::Point) -> Option<(usize, usize)> {
        (0..self.rows.len()).find_map(|row| {
            (0..self.rows[row].len())
                .find(|&index| self.key_area(row, index).contains(point))
                .map(|index| (row, index))
        })
    }

    fn label(&self, env: &Env<'_>, key: Key) -> String {
        match key {
            Key::Char(c) if self.shift => {
                let mut upper = c.to_uppercase();
                match (upper.next(), upper.next()) {
                    (Some(u), None) => u.to_string(),
                    _ => c.to_string(),
                }
            }
            Key::Char(c) => c.to_string(),
            Key::Layer(Layer::Letters) => env.i18n.tr("keyboard.letters-key").to_string(),
            Key::Layer(_) => env.i18n.tr("keyboard.symbols-key").to_string(),
            Key::Done => env.i18n.tr("keyboard.done").to_string(),
            Key::Shift | Key::Delete | Key::Space => String::new(),
        }
    }

    fn press(&mut self, key: Key, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match key {
            Key::Char(c) => {
                let label = self.label(&env, key);
                let typed = if self.shift { label } else { c.to_string() };
                if self.text.chars().count() < MAX_CHARS {
                    self.text.push_str(&typed);
                }
                if self.shift {
                    self.shift = false;
                    self.redraw(cx, self.keys);
                }
            }
            Key::Space => {
                if self.text.chars().count() < MAX_CHARS {
                    self.text.push(' ');
                }
            }
            Key::Delete => {
                self.text.pop();
            }
            Key::Shift => {
                self.shift = !self.shift;
                self.redraw(cx, self.keys);
            }
            Key::Layer(layer) => {
                self.layer = layer;
                self.shift = false;
                self.rows = rows(&env, layer);
                if let Some((row, index)) = self.focus {
                    let row = row.min(self.rows.len() - 1);
                    self.focus = Some((row, index.min(self.rows[row].len() - 1)));
                }
                self.redraw(cx, self.keys);
            }
            Key::Done => {
                self.finish(cx);
                return;
            }
        }
        self.redraw(cx, self.field);
    }

    /// Closes the keyboard and sends the text.
    fn finish(&mut self, cx: &mut Cx<'_, '_>) {
        cx.command(Command::Back);
        if let Some(reply) = self
            .params
            .get(params::REPLY)
            .and_then(|r| Id::parse(r).ok())
        {
            let params = self
                .params
                .clone()
                .with(params::TEXT, core::mem::take(&mut self.text));
            cx.command(Command::Emit(reply, params));
        }
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>, area: Rect) {
        cx.request_redraw_area(area);
        cx.request_refresh(RefreshMode::Fast);
    }

    /// Moves the button focus.
    fn move_focus(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> bool {
        let Some((row, index)) = self.focus else {
            self.focus = Some((0, 0));
            self.redraw(cx, self.key_area(0, 0));
            return true;
        };
        let a = action;
        let last_row = self.rows.len() - 1;
        let to = if *a == actions::LEFT || *a == actions::PREVIOUS {
            match index.checked_sub(1) {
                Some(index) => (row, index),
                None if *a == actions::PREVIOUS => {
                    let row = row.checked_sub(1).unwrap_or(last_row);
                    (row, self.rows[row].len() - 1)
                }
                None => (row, self.rows[row].len() - 1),
            }
        } else if *a == actions::RIGHT || *a == actions::NEXT {
            if index + 1 < self.rows[row].len() {
                (row, index + 1)
            } else if *a == actions::NEXT {
                (if row == last_row { 0 } else { row + 1 }, 0)
            } else {
                (row, 0)
            }
        } else if *a == actions::UP || *a == actions::PREV_PAGE {
            self.nearest(row, index, row.checked_sub(1).unwrap_or(last_row))
        } else if *a == actions::DOWN || *a == actions::NEXT_PAGE {
            self.nearest(row, index, if row == last_row { 0 } else { row + 1 })
        } else {
            return false;
        };
        self.redraw(cx, self.key_area(row, index));
        self.redraw(cx, self.key_area(to.0, to.1));
        self.focus = Some(to);
        true
    }

    /// The key of row `to` nearest below the centre of key `index` of `row`.
    fn nearest(&self, row: usize, index: usize, to: usize) -> (usize, usize) {
        let from = self.key_area(row, index);
        let center = u32::from(from.x) + u32::from(from.width) / 2;
        let at = (0..self.rows[to].len())
            .find(|&i| self.key_area(to, i).right() > center)
            .unwrap_or(self.rows[to].len() - 1);
        (to, at)
    }

    fn draw_field(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let Some(font) = style.font(env) else {
            return;
        };
        let margin = style.margin as f32;
        let f = self.field;
        let title_height = (style.small * 1.5) as u16;
        let title_area = Rect::new(f.x, f.y, f.width, title_height);
        let title = ui::fit(
            &font,
            style.small,
            &self.title,
            f32::from(f.width) - 2.0 * margin,
        );
        ui::draw_label(
            canvas,
            &font,
            style.small,
            margin,
            title_area,
            &title,
            Color::gray(90),
        );
        let m = style.margin / 2;
        let box_area = Rect::new(
            f.x + m as u16,
            f.y + title_height,
            f.width - style.margin as u16,
            f.height - title_height,
        );
        canvas.stroke_rect(
            i32::from(box_area.x),
            i32::from(box_area.y),
            u32::from(box_area.width),
            u32::from(box_area.height),
            style.line.max(2),
            Color::BLACK,
        );
        // The end of the text, with a cursor after it.
        let room = f32::from(box_area.width) - margin * 1.5;
        let mut start = 0;
        while font.measure(style.text, &self.text[start..]) > room {
            start += self.text[start..].chars().next().map_or(1, char::len_utf8);
        }
        let x = f32::from(box_area.x) + margin / 2.0;
        let shown = &self.text[start..];
        ui::draw_label(canvas, &font, style.text, x, box_area, shown, Color::BLACK);
        let end = x + font.measure(style.text, shown);
        let cursor = style.text as u32;
        canvas.fill_rect(
            end as i32 + 2,
            i32::from(box_area.y) + (i32::from(box_area.height) - cursor as i32) / 2,
            style.line.max(2),
            cursor,
            Color::BLACK,
        );
    }

    fn draw_key(&self, canvas: &mut Canvas<'_>, env: &Env<'_>, row: usize, index: usize) {
        let style = self.style;
        let key = self.rows[row][index].0;
        let area = self.key_area(row, index);
        // Buttons as everywhere, with room for the focus frame.
        let h = ui::button_height(area);
        let pad = ui::button_border(&style) as u16 * 3;
        let r = Rect::new(
            area.x + pad,
            area.y + (area.height - h) / 2,
            area.width.saturating_sub(2 * pad),
            h,
        );
        let focused = self.focus == Some((row, index));
        let active = key == Key::Shift && self.shift;
        let label = match key {
            Key::Shift | Key::Delete | Key::Space => String::new(),
            _ => self.label(env, key),
        };
        ui::draw_button(canvas, env, &style, r, &label, active, focused);
        let ink = if active { Color::WHITE } else { Color::BLACK };
        let size = (style.text * 0.6) as i32;
        let (cx, cy) = (
            i32::from(r.x) + i32::from(r.width) / 2,
            i32::from(r.y) + i32::from(r.height) / 2,
        );
        match key {
            Key::Shift => arrow_up(canvas, cx, cy, size, ink),
            Key::Delete => arrow_left(canvas, cx, cy, size, ink),
            Key::Space => {
                let w = i32::from(r.width) / 3;
                let t = style.line.max(2) as i32;
                canvas.fill_rect(cx - w / 2, cy + size / 3, w as u32, t as u32, ink);
                canvas.fill_rect(cx - w / 2, cy, t as u32, (size / 3 + t) as u32, ink);
                canvas.fill_rect(cx + w / 2 - t, cy, t as u32, (size / 3 + t) as u32, ink);
            }
            _ => {}
        }
    }
}

/// An arrow pointing up (*Shift*), centred at `(x, y)`.
fn arrow_up(canvas: &mut Canvas<'_>, x: i32, y: i32, size: i32, color: Color) {
    let half = size / 2;
    for dy in 0..=half {
        canvas.fill_rect(x - dy, y - half + dy, (dy * 2 + 1) as u32, 1, color);
    }
    let stem = (size / 4).max(2);
    canvas.fill_rect(x - stem / 2, y, stem as u32, half as u32, color);
}

/// An arrow pointing left (*Delete*), centred at `(x, y)`.
fn arrow_left(canvas: &mut Canvas<'_>, x: i32, y: i32, size: i32, color: Color) {
    let half = size / 2;
    for dx in 0..=half {
        canvas.fill_rect(x - half + dx, y - dx, 1, (dx * 2 + 1) as u32, color);
    }
    let stem = (size / 4).max(2);
    canvas.fill_rect(x, y - stem / 2, half as u32, stem as u32, color);
}

impl Screen for KeyboardScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            (self.style.line * 2).max(2),
            Color::BLACK,
        );
        self.draw_field(canvas, env);
        let clip = canvas.clip();
        for row in 0..self.rows.len() {
            for index in 0..self.rows[row].len() {
                if self.key_area(row, index).intersection(&clip).is_some() {
                    self.draw_key(canvas, env, row, index);
                }
            }
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if *action == actions::CONFIRM {
            match self.focus {
                Some((row, index)) => self.press(self.rows[row][index].0, cx),
                None => {
                    self.move_focus(action, cx);
                }
            }
            return Handled::Yes;
        }
        if *action == actions::MENU {
            self.finish(cx);
            return Handled::Yes;
        }
        if *action == actions::BACK {
            cx.command(Command::Back);
            return Handled::Yes;
        }
        if self.move_focus(action, cx) {
            Handled::Yes
        } else {
            Handled::No
        }
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if !self.region.contains(gesture.start) {
            return Handled::No;
        }
        let key = self.key_at(gesture.start);
        match (gesture.kind, key) {
            (GestureKind::Tap, Some((row, index))) => {
                if let Some((r, i)) = self.focus.take() {
                    self.redraw(cx, self.key_area(r, i));
                }
                self.press(self.rows[row][index].0, cx);
            }
            // A long press on *Delete* clears the text.
            (GestureKind::LongPress, Some((row, index)))
                if self.rows[row][index].0 == Key::Delete =>
            {
                self.text.clear();
                self.redraw(cx, self.field);
            }
            _ => {}
        }
        Handled::Yes
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

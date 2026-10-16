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
//! - Layouts: the setting *Keyboard layouts* chooses one or more of the
//!   built-in ones (see `layouts`); by default the UI language's, which
//!   a translation brings under `keyboard.letters`. With several, a key
//!   with the layout's code (e.g. `DE`) switches between them.
//! - Accents and other variants: a long press on a letter (with buttons:
//!   a long press on confirm) shows its variants, e.g. `é è ê ë` for `e`,
//!   and German quotes on `"`.
//! - When there is text on the clipboard, *Paste* in the text field
//!   inserts it, and so does a long press in the field (with buttons: up
//!   from the first row).
//! - Where there is a network and a screen for it (slot
//!   `core.remote-keyboard`, e.g. the `transfer` plugin's), *Phone* in the
//!   text field lets one type the text on a phone instead: handy for
//!   passwords and keys.
//!
//! Bind another screen to `core.keyboard` for a different keyboard
//! altogether.

#![no_std]

extern crate alloc;

mod choose;
mod layouts;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::builtin::{actions, groups, params, slots};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params, Plugin,
    Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("keyboard");
pub const SCREEN: Id = Id::new_static("keyboard.screen");
/// The layouts to type with: ids separated by commas, in the order the
/// layout key switches through them.
pub const LAYOUTS_SETTING: Id = Id::new_static("keyboard.layouts");
const CHOOSE_SCREEN: Id = Id::new_static("keyboard.choose");
/// The clipboard's text, asked for by *Paste*.
const PASTED: Id = Id::new_static("keyboard.pasted");
/// The text typed on another device, asked for by *Phone*.
const REMOTE_TEXT: Id = Id::new_static("keyboard.remote-text");

/// Longest text the keyboard accepts, in characters.
const MAX_CHARS: usize = 200;

/// The rows of the symbol layer (the last one also gets *Delete*).
const SYMBOLS: [&str; 3] = ["1234567890", "@#&_-+()/%", "*\"':;!?"];

/// The focus "row" of the buttons in the text field.
const PASTE_ROW: usize = usize::MAX;

/// A button in the text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldButton {
    /// Type the text on another device.
    Phone,
    /// Insert the clipboard's text.
    Paste,
}

impl FieldButton {
    fn label(self) -> &'static str {
        match self {
            FieldButton::Phone => "keyboard.phone",
            FieldButton::Paste => "keyboard.paste",
        }
    }
}

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
        // The layout in use; the next keyboard opens with it.
        let current = Rc::new(Cell::new(0usize));
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("keyboard.title"),
            factory: Box::new(move |env, params| {
                Box::new(KeyboardScreen::new(env, params, current.clone()))
            }),
        })?;
        r.bind_slot(slots::KEYBOARD, SCREEN, Priority::DEFAULT)?;
        r.add_setting(
            SettingSpec::new(
                LAYOUTS_SETTING,
                "keyboard.setting.layouts",
                SettingKind::List,
                SettingValue::Text(Cow::Borrowed(layouts::AUTO)),
            )
            .group(groups::SYSTEM)
            .editor(CHOOSE_SCREEN)
            .description("keyboard.setting.layouts.help"),
        )?;
        r.add_screen(ScreenSpec {
            id: CHOOSE_SCREEN,
            title: Cow::Borrowed("keyboard.setting.layouts"),
            factory: Box::new(|env, _| Box::new(choose::ChooseScreen::new(env))),
        })?;
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
    /// Switches to the next chosen layout.
    NextLayout,
    Done,
}

/// A key and its width in units (a letter is 2).
type Slot = (Key, u8);

/// The rows of a layer; `letters` are the rows of the layout, `switch`
/// adds the key for the next layout.
fn rows(letters: &str, layer: Layer, switch: bool) -> Vec<Vec<Slot>> {
    let chars = |row: &str| -> Vec<Slot> { row.chars().map(|c| (Key::Char(c), 2)).collect() };
    match layer {
        Layer::Letters | Layer::Symbols => {
            let mut rows: Vec<Vec<Slot>> = if layer == Layer::Letters {
                letters.split_whitespace().map(chars).collect()
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
            let mut bottom = alloc::vec![(Key::Layer(other), 3)];
            if switch && layer == Layer::Letters {
                bottom.push((Key::NextLayout, 2));
            }
            let space = if bottom.len() > 1 { 8 } else { 10 };
            bottom.extend([
                (Key::Char(','), 2),
                (Key::Space, space),
                (Key::Char('.'), 2),
                (Key::Done, 3),
            ]);
            rows.push(bottom);
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

/// The variants of a letter, shown above it.
struct Popup {
    chars: Vec<char>,
    area: Rect,
    /// The variant chosen with buttons.
    focus: Option<usize>,
}

impl Popup {
    fn key(&self, index: usize) -> Rect {
        let width = self.area.width / self.chars.len().max(1) as u16;
        Rect::new(
            self.area.x + width * index as u16,
            self.area.y,
            width,
            self.area.height,
        )
    }
}

struct KeyboardScreen {
    /// The opening parameters, sent back with the text.
    params: Params,
    title: String,
    text: String,
    layer: Layer,
    /// The chosen layouts (code, letter rows) and the one in use.
    layouts: Vec<(String, Cow<'static, str>)>,
    current: Rc<Cell<usize>>,
    /// Shown as dots (mode `password`).
    password: bool,
    /// The next letter is a capital.
    shift: bool,
    rows: Vec<Vec<Slot>>,
    /// The key chosen with buttons, as `(row, index)`; none while tapping.
    focus: Option<(usize, usize)>,
    /// The buttons in the text field (*Phone*, *Paste*) where they work,
    /// left to right.
    buttons: Vec<(FieldButton, Rect)>,
    popup: Option<Popup>,
    style: UiStyle,
    region: Rect,
    field: Rect,
    keys: Rect,
    key_height: u16,
}

impl KeyboardScreen {
    fn new(env: &Env<'_>, params: &Params, current: Rc<Cell<usize>>) -> Self {
        let style = UiStyle::new(env);
        let layer = if params.get(params::MODE) == Some("number") {
            Layer::Digits
        } else {
            Layer::Letters
        };
        let layouts = layouts::chosen(env, env.settings.text(LAYOUTS_SETTING.as_str()));
        if current.get() >= layouts.len() {
            current.set(0);
        }
        let rows = rows(&layouts[current.get()].1, layer, layouts.len() > 1);
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
        let mut screen = Self {
            title: params.get(params::TITLE).unwrap_or_default().to_string(),
            text: params.get(params::TEXT).unwrap_or_default().to_string(),
            params: params.clone(),
            password: params.get(params::MODE) == Some("password"),
            layer,
            layouts,
            current,
            shift: false,
            rows,
            focus: (!env.device.input.touch).then_some((0, 0)),
            buttons: Vec::new(),
            popup: None,
            style,
            region,
            field,
            keys,
            key_height,
        };
        screen.buttons = screen.field_buttons(env);
        screen
    }

    /// The box of the text field: on the margins, like its title above
    /// and the keys below.
    fn text_box(&self) -> Rect {
        let f = self.field;
        let title_height = (self.style.small * 1.5) as u16;
        let m = self.style.margin as u16;
        Rect::new(
            f.x + m,
            f.y + title_height,
            f.width.saturating_sub(2 * m),
            f.height - title_height,
        )
    }

    /// Keys are drawn inset by this inside their area (room for the
    /// focus frame), so their areas reach this far over the margins.
    fn key_inset(&self) -> u32 {
        ui::button_border(&self.style) * 3
    }

    /// The buttons at the right end of the text field: *Phone* where text
    /// can come from another device, *Paste* when there is text on the
    /// clipboard.
    fn field_buttons(&self, env: &Env<'_>) -> Vec<(FieldButton, Rect)> {
        let Some(font) = self.style.font(env) else {
            return Vec::new();
        };
        let mut kinds = Vec::new();
        if env.network && env.has_slot(&slots::REMOTE_KEYBOARD) {
            kinds.push(FieldButton::Phone);
        }
        if env.clipboard {
            kinds.push(FieldButton::Paste);
        }
        let b = self.text_box();
        let height = ui::button_height(b);
        let pad = (b.height - height) / 2;
        // From the right: each as wide as its label (rounded up) and the
        // button's padding.
        let mut right = b.x + b.width - pad;
        let mut buttons: Vec<(FieldButton, Rect)> = kinds
            .into_iter()
            .rev()
            .map(|kind| {
                let label = env.i18n.tr(kind.label());
                let label_width = (font.measure(self.style.small, label) + 1.0) as u16;
                let width = label_width + self.style.gap as u16 * 3;
                let area = Rect::new(right - width, b.y + pad, width, height);
                right -= width + pad;
                (kind, area)
            })
            .collect();
        buttons.reverse();
        buttons
    }

    /// What a field button does.
    fn field_button(&mut self, kind: FieldButton, cx: &mut Cx<'_, '_>) {
        match kind {
            FieldButton::Paste => self.paste(cx),
            FieldButton::Phone => {
                // The text so far goes along, but never a password.
                let text = if self.password {
                    String::new()
                } else {
                    self.text.clone()
                };
                let mut params = Params::new()
                    .with(params::TITLE, self.title.clone())
                    .with(params::TEXT, text)
                    .with(params::REPLY, REMOTE_TEXT.as_str());
                if self.password {
                    params = params.with(params::MODE, "password");
                }
                cx.command(Command::OpenSlot(slots::REMOTE_KEYBOARD, params));
            }
        }
    }

    /// Where key `index` of row `row` is (the field's buttons for
    /// [`PASTE_ROW`]).
    fn key_area(&self, row: usize, index: usize) -> Rect {
        if row == PASTE_ROW {
            return self.buttons.get(index).map_or(self.field, |b| b.1);
        }
        let keys = &self.rows[row];
        let total: u32 = keys.iter().map(|(_, w)| u32::from(*w)).sum::<u32>().max(1);
        // The keys' visible edges on the margins.
        let outside = self.style.margin.saturating_sub(self.key_inset());
        let width = u32::from(self.keys.width) - 2 * outside;
        let x0 = u32::from(self.keys.x) + outside;
        let before: u32 = keys[..index].iter().map(|(_, w)| u32::from(*w)).sum();
        let left = x0 + width * before / total;
        let right = x0 + width * (before + u32::from(keys[index].1)) / total;
        let y = self.keys.y + self.key_height * row as u16;
        Rect::new(left as u16, y, (right - left) as u16, self.key_height)
    }

    fn key_at(&self, point: quireink_sdk::Point) -> Option<(usize, usize)> {
        if let Some(index) = self.buttons.iter().position(|b| b.1.contains(point)) {
            return Some((PASTE_ROW, index));
        }
        (0..self.rows.len()).find_map(|row| {
            (0..self.rows[row].len())
                .find(|&index| self.key_area(row, index).contains(point))
                .map(|index| (row, index))
        })
    }

    /// `c` as typed now: a capital after *Shift* (if it has one capital).
    fn cased(&self, c: char) -> char {
        if !self.shift {
            return c;
        }
        let mut upper = c.to_uppercase();
        match (upper.next(), upper.next()) {
            (Some(u), None) => u,
            _ => c,
        }
    }

    fn label(&self, env: &Env<'_>, key: Key) -> String {
        match key {
            Key::Char(c) => self.cased(c).to_string(),
            Key::Layer(Layer::Letters) => env.i18n.tr("keyboard.letters-key").to_string(),
            Key::Layer(_) => env.i18n.tr("keyboard.symbols-key").to_string(),
            Key::NextLayout => self.layouts[self.current.get()].0.clone(),
            Key::Done => env.i18n.tr("keyboard.done").to_string(),
            Key::Shift | Key::Delete | Key::Space => String::new(),
        }
    }

    /// Adds text at the end, as far as there is room.
    fn insert(&mut self, text: &str, cx: &mut Cx<'_, '_>) {
        let room = MAX_CHARS.saturating_sub(self.text.chars().count());
        self.text.extend(text.chars().take(room));
        if self.shift {
            self.shift = false;
            self.redraw(cx, self.keys);
        }
        self.redraw(cx, self.field);
    }

    fn press(&mut self, key: Key, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match key {
            Key::Char(c) => {
                let c = self.cased(c);
                let mut buffer = [0u8; 4];
                return self.insert(c.encode_utf8(&mut buffer), cx);
            }
            Key::Space => return self.insert(" ", cx),
            Key::Delete => {
                self.text.pop();
            }
            Key::Shift => {
                self.shift = !self.shift;
                self.redraw(cx, self.keys);
            }
            Key::Layer(layer) => {
                self.layer = layer;
                self.rebuild(&env, cx);
            }
            Key::NextLayout => {
                self.current
                    .set((self.current.get() + 1) % self.layouts.len());
                self.rebuild(&env, cx);
            }
            Key::Done => {
                self.finish(cx);
                return;
            }
        }
        self.redraw(cx, self.field);
    }

    /// The keys again, after the layer or layout changed.
    fn rebuild(&mut self, env: &Env<'_>, cx: &mut Cx<'_, '_>) {
        let _ = env;
        self.shift = false;
        let letters = self.layouts[self.current.get()].1.clone();
        self.rows = rows(&letters, self.layer, self.layouts.len() > 1);
        if let Some((row, index)) = self.focus.filter(|(row, _)| *row != PASTE_ROW) {
            let row = row.min(self.rows.len() - 1);
            self.focus = Some((row, index.min(self.rows[row].len() - 1)));
        }
        self.redraw(cx, self.keys);
    }

    /// Asks for the clipboard's text ([`PASTED`] brings it).
    fn paste(&mut self, cx: &mut Cx<'_, '_>) {
        cx.command(Command::ReadClipboard(PASTED, Params::new()));
    }

    /// Inserts pasted text: one line, digits only for numbers.
    fn pasted(&mut self, text: &str, cx: &mut Cx<'_, '_>) {
        let line: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .filter(|c| self.layer != Layer::Digits || c.is_ascii_digit())
            .collect();
        self.insert(line.trim(), cx);
    }

    /// Shows the variants of the letter at `(row, index)`, if it has any.
    fn open_variants(&mut self, row: usize, index: usize, cx: &mut Cx<'_, '_>) -> bool {
        let Some(Key::Char(c)) = self.rows.get(row).and_then(|r| r.get(index)).map(|k| k.0) else {
            return false;
        };
        let Some(variants) = layouts::variants(c) else {
            return false;
        };
        let chars: Vec<char> = variants.chars().map(|v| self.cased(v)).collect();
        let key = self.key_area(row, index);
        let margin = self.style.margin as u16 / 2;
        let room = self.region.width - 2 * margin;
        let width = (key.width.max(self.key_height) * chars.len() as u16).min(room);
        let center = key.x + key.width / 2;
        let x = center
            .saturating_sub(width / 2)
            .clamp(margin, self.region.width - margin - width);
        let y = key.y.saturating_sub(self.key_height).max(self.region.y);
        let focus = self.focus.map(|_| 0);
        self.popup = Some(Popup {
            chars,
            area: Rect::new(x, y, width, self.key_height),
            focus,
        });
        self.redraw(cx, self.region);
        true
    }

    fn close_variants(&mut self, cx: &mut Cx<'_, '_>) {
        if self.popup.take().is_some() {
            self.redraw(cx, self.region);
        }
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
        let up = *a == actions::UP || *a == actions::PREV_PAGE;
        let down = *a == actions::DOWN || *a == actions::NEXT_PAGE;
        let to = if row == PASTE_ROW {
            // The field's buttons sit above the first row.
            let last = self.buttons.len().saturating_sub(1);
            if down || *a == actions::NEXT && index == last {
                self.nearest(row, index, 0)
            } else if up {
                self.nearest(row, index, last_row)
            } else if *a == actions::RIGHT || *a == actions::NEXT {
                (row, (index + 1).min(last))
            } else if *a == actions::LEFT || *a == actions::PREVIOUS {
                (row, index.saturating_sub(1))
            } else {
                return false;
            }
        } else if *a == actions::LEFT || *a == actions::PREVIOUS {
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
        } else if up {
            match row.checked_sub(1) {
                Some(above) => self.nearest(row, index, above),
                None if !self.buttons.is_empty() => (PASTE_ROW, self.buttons.len() - 1),
                None => self.nearest(row, index, last_row),
            }
        } else if down {
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

    /// Buttons while the variants are shown.
    fn on_variants_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let Some(popup) = &mut self.popup else {
            return Handled::No;
        };
        let last = popup.chars.len() - 1;
        let focus = popup.focus.unwrap_or(0);
        let a = action;
        let to = if [actions::LEFT, actions::PREVIOUS, actions::UP].contains(a) {
            focus.saturating_sub(1)
        } else if [actions::RIGHT, actions::NEXT, actions::DOWN].contains(a) {
            (focus + 1).min(last)
        } else if *a == actions::CONFIRM {
            let c = popup.chars[focus];
            self.close_variants(cx);
            let mut buffer = [0u8; 4];
            self.insert(c.encode_utf8(&mut buffer), cx);
            return Handled::Yes;
        } else if *a == actions::BACK || *a == actions::MENU {
            self.close_variants(cx);
            return Handled::Yes;
        } else {
            return Handled::Yes;
        };
        popup.focus = Some(to);
        let area = popup.area;
        self.redraw(cx, area);
        Handled::Yes
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
            style.faint(90),
        );
        let box_area = self.text_box();
        ui::stroke_round(canvas, &style, box_area, style.line.max(2), Color::BLACK);
        // The end of the text, with a cursor after it; a password as dots.
        let text = if self.password {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        };
        let buttons_width = self.buttons.first().map_or(0.0, |(_, area)| {
            f32::from(box_area.x + box_area.width - area.x) + margin
        });
        let room = f32::from(box_area.width) - margin * 1.5 - buttons_width;
        let mut start = 0;
        while font.measure(style.text, &text[start..]) > room {
            start += text[start..].chars().next().map_or(1, char::len_utf8);
        }
        let x = f32::from(box_area.x) + margin / 2.0;
        let shown = &text[start..];
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
        for (index, &(kind, area)) in self.buttons.iter().enumerate() {
            let focused = self.focus == Some((PASTE_ROW, index));
            let label = env.i18n.tr(kind.label());
            ui::draw_button(canvas, env, &style, area, label, false, focused);
        }
    }

    fn draw_key(&self, canvas: &mut Canvas<'_>, env: &Env<'_>, row: usize, index: usize) {
        let style = self.style;
        let key = self.rows[row][index].0;
        let area = self.key_area(row, index);
        // Buttons as everywhere, with room for the focus frame.
        let h = ui::button_height(area);
        let pad = self.key_inset() as u16;
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

    fn draw_variants(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let Some(popup) = &self.popup else {
            return;
        };
        let style = self.style;
        let a = popup.area;
        let border = (style.line * 2).max(2);
        let out = border as u16 * 2;
        let frame = Rect::new(
            a.x.saturating_sub(out),
            a.y.saturating_sub(out),
            a.width + 2 * out,
            a.height + 2 * out,
        );
        ui::fill(canvas, frame, Color::WHITE);
        canvas.stroke_rect(
            i32::from(frame.x),
            i32::from(frame.y),
            u32::from(frame.width),
            u32::from(frame.height),
            border,
            Color::BLACK,
        );
        for (index, c) in popup.chars.iter().enumerate() {
            let key = popup.key(index);
            let h = ui::button_height(key);
            let pad = ui::button_border(&style) as u16 * 3;
            let r = Rect::new(
                key.x + pad,
                key.y + (key.height - h) / 2,
                key.width.saturating_sub(2 * pad),
                h,
            );
            let focused = popup.focus == Some(index);
            ui::draw_button(canvas, env, &style, r, &c.to_string(), false, focused);
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
        self.draw_variants(canvas, env);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if self.popup.is_some() {
            return self.on_variants_action(action, cx);
        }
        if *action == actions::CONFIRM {
            match self.focus {
                Some((PASTE_ROW, index)) => {
                    if let Some(&(kind, _)) = self.buttons.get(index) {
                        self.field_button(kind, cx);
                    }
                }
                Some((row, index)) => self.press(self.rows[row][index].0, cx),
                None => {
                    self.move_focus(action, cx);
                }
            }
            return Handled::Yes;
        }
        if *action == actions::MENU {
            // A long press on confirm: the variants of a letter, else done.
            let opened = self
                .focus
                .is_some_and(|(row, index)| self.open_variants(row, index, cx));
            if !opened {
                self.finish(cx);
            }
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

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == PASTED {
            self.pasted(params.get(params::TEXT).unwrap_or_default(), cx);
        } else if *event == REMOTE_TEXT {
            // Typed on the phone with the text so far: it replaces it.
            self.text.clear();
            self.pasted(params.get(params::TEXT).unwrap_or_default(), cx);
        }
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if !self.region.contains(gesture.start) {
            return Handled::No;
        }
        if let Some(popup) = &self.popup {
            // A variant, or anywhere else to close them.
            let chosen = (0..popup.chars.len())
                .find(|&i| popup.key(i).contains(gesture.start))
                .map(|i| popup.chars[i]);
            self.close_variants(cx);
            if let Some(c) = chosen.filter(|_| gesture.kind == GestureKind::Tap) {
                let mut buffer = [0u8; 4];
                self.insert(c.encode_utf8(&mut buffer), cx);
            }
            return Handled::Yes;
        }
        let key = self.key_at(gesture.start);
        if let Some((r, i)) = self.focus.take() {
            self.redraw(cx, self.key_area(r, i));
        }
        match (gesture.kind, key) {
            (GestureKind::Tap, Some((PASTE_ROW, index))) => {
                if let Some(&(kind, _)) = self.buttons.get(index) {
                    self.field_button(kind, cx);
                }
            }
            // A long press in the text field pastes too, as elsewhere.
            (GestureKind::LongPress, _)
                if self.field.contains(gesture.start)
                    && self.buttons.iter().any(|b| b.0 == FieldButton::Paste) =>
            {
                self.paste(cx);
            }
            (GestureKind::Tap, Some((row, index))) => self.press(self.rows[row][index].0, cx),
            // A long press on *Delete* clears the text.
            (GestureKind::LongPress, Some((row, index)))
                if row != PASTE_ROW && self.rows[row][index].0 == Key::Delete =>
            {
                self.text.clear();
                self.redraw(cx, self.field);
            }
            // A long press on a letter shows its variants.
            (GestureKind::LongPress, Some((row, index))) if row != PASTE_ROW => {
                self.open_variants(row, index, cx);
            }
            _ => {}
        }
        Handled::Yes
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    use super::*;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn several_layouts_get_a_key_to_switch() {
        let letters = "abc def ghi";
        let one = rows(letters, Layer::Letters, false);
        let two = rows(letters, Layer::Letters, true);
        let has = |rows: &Vec<Vec<Slot>>| rows.iter().flatten().any(|(k, _)| *k == Key::NextLayout);
        assert!(!has(&one));
        assert!(has(&two));
        // Rows add up to the same width with or without it.
        let width = |rows: &Vec<Vec<Slot>>| -> u32 {
            rows.last()
                .unwrap()
                .iter()
                .map(|(_, w)| u32::from(*w))
                .sum()
        };
        assert_eq!(width(&one), width(&two));
        // Not on the symbols.
        assert!(!has(&rows(letters, Layer::Symbols, true)));
    }
}

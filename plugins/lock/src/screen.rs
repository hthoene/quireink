//! The lock screen, for unlocking and for setting the lock up.
//!
//! - A PIN on a keypad (digits, delete, OK), a pattern through three by
//!   three dots (drawn with a finger, or chosen dot by dot with the
//!   buttons and *OK*), a password on the keyboard.
//! - Unlocking (parameter `mode` = `unlock`) takes every input until the
//!   code is right, and opens again if something closes it.
//! - Setting up (as the setting's editor): the code in use first, then the
//!   kind, then the new code twice.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::semantics::Role;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Instant, Metrics,
    Params, Point, PointerEvent, PointerPhase, Rect, RefreshMode, Screen, SettingValue,
};

use crate::{KIND, KIND_PREFIX, KINDS, Locked, SCREEN, code};

/// Parameter: what the screen is for; [`UNLOCK`] to unlock, else setting
/// up.
pub(crate) const MODE: &str = "mode";
pub(crate) const UNLOCK: &str = "unlock";
/// Reply: the password typed.
const TYPED: Id = Id::new_static("lock.typed");

/// The shortest code.
const MIN_LENGTH: usize = 4;
/// Wrong tries before the next one waits, and how long.
const TRIES: u8 = 5;
const WAIT_MS: u64 = 30_000;
/// The keypad of a PIN, row by row.
const KEYS: [&str; 12] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", DELETE, "0", "OK",
];
/// The key that deletes the last digit.
const DELETE: &str = "delete";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// The code to unlock.
    Unlock,
    /// The code in use, before it is changed.
    Verify,
    /// Which kind of lock, listed.
    Choose,
    /// A new code, then the same again.
    New,
    Confirm(String),
}

pub(crate) struct LockScreen {
    locked: Locked,
    step: Step,
    /// What is entered: `pin`, `pattern` or `password`.
    kind: String,
    /// The digits of a PIN, or the dots of a pattern (`0` to `8`).
    entered: String,
    message: Option<String>,
    failures: u8,
    wait_until: Option<Instant>,
    /// The key or dot chosen with the buttons (the pattern's *OK* is 9).
    focus: usize,
    shown: bool,
    /// The finger drawing a pattern, and where it is.
    finger: Option<(u8, Point)>,
    /// Whether the keyboard was opened already.
    asked: bool,
    style: UiStyle,
    header: ui::Header,
    layout: Layout,
}

/// Where things are: as on a phone, the clock (when unlocking) or the
/// header (when setting up) at the top, then what is asked for, the
/// code's dots and a message, and the keypad or pattern below.
struct Layout {
    /// The clock and the date, when unlocking.
    clock: Rect,
    clock_size: f32,
    /// What is asked for, when there is no header to say it.
    title: Rect,
    /// The dots that stand for a PIN's digits.
    dots: Rect,
    message: Rect,
    /// Where the kinds are listed.
    list_top: u16,
    keys: [Rect; 12],
    /// The centres of the pattern's dots, their radius, its *OK*.
    points: [Point; 9],
    radius: i32,
    ok: Rect,
    /// The password's button.
    password: Rect,
}

/// The height of a line of text at `size`.
fn line(env: &Env<'_>, style: &UiStyle, size: f32) -> u16 {
    style
        .font(env)
        .map_or(size * 1.3, |f| f.line_metrics(size).line_height) as u16
}

impl Layout {
    fn new(env: &Env<'_>, style: &UiStyle, header: bool) -> Self {
        let display = env.device.display;
        let (w, h) = (display.width, display.height);
        let metrics = Metrics::new(display.dpi);
        let row = style.row as u16;
        let gap = style.gap as u16;
        let mut y = if header {
            style.header as u16 + gap * 2
        } else {
            h / 12
        };
        let list_top = y;
        let clock_size = style.title * 2.6;
        let clock = if header {
            Rect::new(0, y, w, 0)
        } else {
            let height = line(env, style, clock_size) + line(env, style, style.small);
            Rect::new(0, y, w, height)
        };
        y += clock.height + if header { 0 } else { gap * 3 };
        let title = Rect::new(0, y, w, if header { 0 } else { row });
        y += title.height;
        let dots = Rect::new(0, y, w, row);
        y += row;
        let message = Rect::new(0, y, w, row);
        y += row + gap;
        // The keypad at the bottom: round keys, at most 15 mm across,
        // three a row with room between them.
        let bottom = h.saturating_sub((h / 14).max(style.margin as u16 * 2));
        let room = bottom.saturating_sub(y);
        let width = w.saturating_sub(style.margin as u16 * 4);
        let d = (metrics.mm(15) as u16)
            .min(room * 4 / 19)
            .min(width / 4)
            .max(8);
        let (across, down) = (d * 3 / 2, d * 5 / 4);
        let pad_top = bottom.saturating_sub(down * 3 + d);
        let keys = core::array::from_fn(|i| {
            let (col, line) = ((i % 3) as u16, (i / 3) as u16);
            Rect::new(
                w / 2 - d / 2 - across + col * across,
                pad_top + line * down,
                d,
                d,
            )
        });
        // The pattern: three by three dots in the same room, OK below
        // where there is no touch.
        let ok_h = ui::button_height(Rect::new(0, 0, 0, row));
        let ok_room = if env.device.input.touch {
            0
        } else {
            ok_h + row
        };
        let step = (metrics.mm(22) as u16)
            .min(width / 3)
            .min(room.saturating_sub(ok_room) / 3)
            .max(3);
        let x0 = (w - step * 3) / 2;
        let y0 = y + (room.saturating_sub(ok_room + step * 3)) / 2;
        let points = core::array::from_fn(|i| {
            Point::new(
                i32::from(x0 + (i % 3) as u16 * step + step / 2),
                i32::from(y0 + (i / 3) as u16 * step + step / 2),
            )
        });
        let ok_w = w / 3;
        let ok = Rect::new((w - ok_w) / 2, y0 + step * 3 + row / 2, ok_w, ok_h);
        let password = Rect::new(w / 6, y + room.saturating_sub(ok_h) / 3, w * 2 / 3, ok_h);
        Self {
            clock,
            clock_size,
            title,
            dots,
            message,
            list_top,
            keys,
            points,
            radius: i32::from(step / 10).max(4),
            ok,
            password,
        }
    }

    /// The pattern's dot at `point`, if near one.
    fn dot_at(&self, point: Point) -> Option<usize> {
        let reach = self.radius * 3;
        self.points.iter().position(|p| {
            let (dx, dy) = (p.x - point.x, p.y - point.y);
            dx * dx + dy * dy <= reach * reach
        })
    }
}

impl LockScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params, locked: Locked) -> Self {
        let style = UiStyle::new(env);
        let active = code::active(env);
        let step = if params.get(MODE) == Some(UNLOCK) {
            Step::Unlock
        } else if active.is_some() {
            Step::Verify
        } else {
            Step::Choose
        };
        let header = step != Step::Unlock;
        Self {
            locked,
            header: ui::Header::new(env, &style, header),
            step,
            kind: active.unwrap_or_else(|| String::from("pin")),
            entered: String::new(),
            message: None,
            failures: 0,
            wait_until: None,
            focus: 0,
            shown: !env.device.input.touch,
            finger: None,
            asked: false,
            layout: Layout::new(env, &style, header),
            style,
        }
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn waiting(&self, env: &Env<'_>) -> bool {
        self.wait_until.is_some_and(|until| env.now < until)
    }

    /// Asks for the password on the keyboard.
    fn ask_password(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        self.asked = true;
        cx.command(Command::OpenSlot(
            slots::KEYBOARD,
            Params::new()
                .with(params::TITLE, env.i18n.tr(self.title_key()).to_string())
                .with(params::MODE, "password")
                .with(params::REPLY, TYPED.as_str()),
        ));
    }

    /// The kinds offered; patterns only where fingers draw them.
    fn kinds(env: &Env<'_>) -> Vec<&'static str> {
        KINDS
            .iter()
            .copied()
            .filter(|k| *k != "pattern" || env.device.input.touch)
            .collect()
    }

    /// Where kind `index` is listed.
    fn kind_row(&self, index: usize) -> Rect {
        let l = &self.layout;
        let row = self.style.row as u16;
        Rect::new(0, l.list_top + index as u16 * row, l.dots.width, row)
    }

    /// Kind `kind` was chosen: off at once, else the new code.
    fn choose_kind(&mut self, kind: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if kind == "off" {
            cx.command(code::forget());
            cx.command(Command::SetSetting(
                KIND,
                SettingValue::Text("lock.kind.off".into()),
            ));
            cx.command(Command::Notify(env.i18n.tr("lock.removed").to_string()));
            cx.command(Command::Back);
            return;
        }
        self.kind = kind.to_string();
        self.step = Step::New;
        self.entered.clear();
        self.focus = 0;
        self.asked = false;
        if self.kind == "password" {
            self.ask_password(cx);
        }
        self.redraw(cx);
    }

    fn title_key(&self) -> &'static str {
        match (&self.step, self.kind.as_str()) {
            (Step::Choose, _) => "lock.setting.kind",
            (Step::Confirm(_), _) => "lock.again",
            (Step::New, "pattern") => "lock.new-pattern",
            (Step::New, "password") => "lock.new-password",
            (Step::New, _) => "lock.new-pin",
            (_, "pattern") => "lock.draw-pattern",
            (_, "password") => "lock.enter-password",
            _ => "lock.enter-pin",
        }
    }

    /// A code was entered.
    fn submit(&mut self, entered: String, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        self.entered.clear();
        self.finger = None;
        match self.step.clone() {
            Step::Unlock | Step::Verify => {
                if !code::check(&env, &entered) {
                    self.failures += 1;
                    if self.failures >= TRIES {
                        self.failures = 0;
                        let until = env.now.add_millis(WAIT_MS);
                        self.wait_until = Some(until);
                        cx.wake_at(until);
                    }
                    self.message = Some(env.i18n.tr("lock.wrong").to_string());
                } else if self.step == Step::Unlock {
                    self.locked.set(false);
                    cx.command(Command::Back);
                    return;
                } else {
                    self.step = Step::Choose;
                    self.focus = 0;
                    self.message = None;
                }
            }
            Step::New => {
                if entered.chars().count() < MIN_LENGTH {
                    let text = env
                        .i18n
                        .format("lock.too-short", &[("count", Arg::Int(MIN_LENGTH as i64))]);
                    self.message = Some(text);
                } else {
                    self.step = Step::Confirm(entered);
                    self.message = None;
                    self.asked = false;
                }
            }
            Step::Confirm(first) => {
                if entered == first {
                    cx.command(code::keep(&env, &self.kind, &entered));
                    cx.command(Command::SetSetting(
                        KIND,
                        SettingValue::Text(alloc::format!("{KIND_PREFIX}{}", self.kind).into()),
                    ));
                    cx.command(Command::Notify(env.i18n.tr("lock.done").to_string()));
                    cx.command(Command::Back);
                    return;
                }
                self.step = Step::New;
                self.asked = false;
                self.message = Some(env.i18n.tr("lock.mismatch").to_string());
            }
            Step::Choose => {}
        }
        if self.kind == "password" && !self.asked && self.step != Step::Choose {
            self.ask_password(cx);
        }
        self.redraw(cx);
    }

    /// A key of the PIN's keypad was pressed.
    fn press(&mut self, key: usize, cx: &mut Cx<'_, '_>) {
        match KEYS.get(key) {
            Some(&DELETE) => {
                self.entered.pop();
            }
            Some(&"OK") => {
                let entered = core::mem::take(&mut self.entered);
                return self.submit(entered, cx);
            }
            Some(digit) if self.entered.len() < 16 => self.entered.push_str(digit),
            _ => {}
        }
        self.message = None;
        self.redraw(cx);
    }

    /// Adds dot `dot` to the pattern, once.
    fn add_dot(&mut self, dot: usize) -> bool {
        let c = char::from(b'0' + dot as u8);
        if self.entered.contains(c) {
            return false;
        }
        self.entered.push(c);
        true
    }

    /// The things chosen with the buttons, in order.
    fn targets(&self, env: &Env<'_>) -> usize {
        match self.kind.as_str() {
            "pattern" if env.device.input.touch => 9,
            "pattern" => 10,
            "password" => 1,
            _ => KEYS.len(),
        }
    }

    fn choose(&mut self, cx: &mut Cx<'_, '_>) {
        match self.kind.as_str() {
            "pattern" if self.focus == 9 => {
                let entered = core::mem::take(&mut self.entered);
                self.submit(entered, cx);
            }
            "pattern" => {
                self.add_dot(self.focus);
                self.redraw(cx);
            }
            "password" => self.ask_password(cx),
            _ => self.press(self.focus, cx),
        }
    }

    fn draw_pin(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = &self.style;
        let l = &self.layout;
        // A dot for every digit, rings for the least still to come.
        let r = i32::from(l.dots.height) / 7;
        let count = self.entered.len().max(MIN_LENGTH) as i32;
        let gap = r * 4;
        let mut x = i32::from(l.dots.width) / 2 - (count - 1) * gap / 2;
        let y = i32::from(l.dots.y) + i32::from(l.dots.height) / 2;
        for i in 0..count {
            if (i as usize) < self.entered.len() {
                circle(canvas, Point::new(x, y), r, Color::BLACK);
            } else {
                ring(canvas, Point::new(x, y), r, style.line.max(2));
            }
            x += gap;
        }
        let Some(font) = style.font(env) else {
            return;
        };
        for (i, key) in KEYS.iter().enumerate() {
            let rect = l.keys[i];
            let focused = self.shown && self.focus == i;
            let centre = Point::new(
                i32::from(rect.x) + i32::from(rect.width) / 2,
                i32::from(rect.y) + i32::from(rect.height) / 2,
            );
            let radius = i32::from(rect.width) / 2;
            canvas.describe(Role::Button, key, rect, focused, None);
            // Digits in a thin ring; delete and OK as plain signs. Chosen
            // with the buttons, a key is filled.
            let digit = *key != DELETE && *key != "OK";
            let ink = if focused {
                circle(canvas, centre, radius, Color::BLACK);
                Color::WHITE
            } else {
                if digit {
                    ring(canvas, centre, radius, style.line.max(1));
                }
                Color::BLACK
            };
            if digit {
                let size = f32::from(rect.height) * 0.42;
                let x = f32::from(rect.x) + (f32::from(rect.width) - font.measure(size, key)) / 2.0;
                ui::draw_label(canvas, &font, size, x, rect, key, ink);
            } else {
                let name = if *key == DELETE {
                    "arrow-left"
                } else {
                    "check"
                };
                let size = f32::from(rect.height) * 0.4;
                let x = f32::from(rect.x)
                    + (f32::from(rect.width) - ui::icon_width(env, name, size)) / 2.0;
                ui::draw_icon(canvas, env, name, size, x, rect, ink);
            }
        }
    }

    /// The time, large, and the date below it.
    fn draw_clock(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = &self.style;
        let l = &self.layout;
        let Some(font) = style.font(env) else {
            return;
        };
        let time = quireink_sdk::time::clock_text(env).unwrap_or_default();
        let big = line(env, style, l.clock_size);
        let top = Rect::new(l.clock.x, l.clock.y, l.clock.width, big);
        let x = (f32::from(top.width) - font.measure(l.clock_size, &time)) / 2.0;
        ui::draw_label(canvas, &font, l.clock_size, x, top, &time, Color::BLACK);
        if let Some(date) = quireink_sdk::time::date_text(env) {
            let below = Rect::new(top.x, top.y + big, top.width, line(env, style, style.small));
            let x = (f32::from(below.width) - font.measure(style.small, &date)) / 2.0;
            ui::draw_label(canvas, &font, style.small, x, below, &date, style.faint(70));
        }
    }

    fn draw_pattern(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let l = &self.layout;
        let line = (l.radius / 2).max(2) as u32;
        let chosen: Vec<usize> = self
            .entered
            .bytes()
            .map(|b| usize::from(b - b'0'))
            .collect();
        for pair in chosen.windows(2) {
            canvas.draw_line(l.points[pair[0]], l.points[pair[1]], line, Color::BLACK);
        }
        if let (Some(&last), Some((_, at))) = (chosen.last(), self.finger) {
            canvas.draw_line(l.points[last], at, line, Color::gray(110));
        }
        for (i, &point) in l.points.iter().enumerate() {
            canvas.describe(
                Role::Button,
                "",
                Rect::new(
                    (point.x - l.radius * 3).max(0) as u16,
                    (point.y - l.radius * 3).max(0) as u16,
                    (l.radius * 6) as u16,
                    (l.radius * 6) as u16,
                ),
                self.shown && self.focus == i,
                None,
            );
            let on = chosen.contains(&i);
            circle(
                canvas,
                point,
                if on { l.radius } else { l.radius / 2 },
                Color::BLACK,
            );
            if on {
                ring(canvas, point, l.radius * 2, self.style.line.max(1));
            }
            if self.shown && self.focus == i {
                ring(canvas, point, l.radius * 3, self.style.line * 2);
            }
        }
        if !env.device.input.touch {
            let ok = env.i18n.tr("lock.ok");
            let focused = self.shown && self.focus == 9;
            ui::draw_button(canvas, env, &self.style, l.ok, ok, false, focused);
        }
    }
}

/// A filled circle.
fn circle(canvas: &mut Canvas<'_>, at: Point, r: i32, color: Color) {
    let d = (r * 2) as u32;
    canvas.fill_round_rect(at.x - r, at.y - r, d, d, r as u32, color);
}

/// A circle's outline.
fn ring(canvas: &mut Canvas<'_>, at: Point, r: i32, width: u32) {
    let d = (r * 2) as u32;
    canvas.stroke_round_rect(at.x - r, at.y - r, d, d, width, r as u32, Color::BLACK);
}

impl Screen for LockScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        canvas.clear(Color::WHITE);
        let style = self.style;
        let title = env.i18n.tr(self.title_key());
        if self.step == Step::Unlock {
            self.draw_clock(canvas, env);
            ui::draw_message(canvas, env, &style, self.layout.title, title);
        } else {
            self.header.draw(canvas, env, &style, title, None, None);
        }
        let message = if self.waiting(env) {
            let seconds = self.wait_until.map_or(0, |until| {
                until.millis().saturating_sub(env.now.millis()) / 1000 + 1
            });
            Some(
                env.i18n
                    .format("lock.wait", &[("count", Arg::Int(seconds as i64))]),
            )
        } else if self.message.is_none() && self.step == Step::New {
            // What a new code needs.
            let count = Arg::Int(MIN_LENGTH as i64);
            Some(env.i18n.format("lock.too-short", &[("count", count)]))
        } else {
            self.message.clone()
        };
        if let Some(message) = message {
            ui::draw_message(canvas, env, &style, self.layout.message, &message);
        }
        if self.step == Step::Choose {
            let current = code::active(env).unwrap_or_else(|| String::from("off"));
            for (i, kind) in Self::kinds(env).into_iter().enumerate() {
                let name = env
                    .i18n
                    .tr(&alloc::format!("{KIND_PREFIX}{kind}"))
                    .to_string();
                let focused = self.shown && self.focus == i;
                let row = self.kind_row(i);
                ui::draw_value_row(canvas, env, &style, row, focused, &name, "");
                if kind == current {
                    let size = style.text;
                    let x = f32::from(row.x + row.width)
                        - style.margin as f32
                        - ui::icon_width(env, "check", size);
                    let ink = if focused { Color::WHITE } else { Color::BLACK };
                    ui::draw_icon(canvas, env, "check", size, x, row, ink);
                }
            }
            return;
        }
        match self.kind.as_str() {
            "pattern" => self.draw_pattern(canvas, env),
            "password" => {
                let label = env.i18n.tr("lock.type-password");
                let focused = self.shown;
                ui::draw_button(
                    canvas,
                    env,
                    &style,
                    self.layout.password,
                    label,
                    false,
                    focused,
                );
            }
            _ => self.draw_pin(canvas, env),
        }
    }

    /// Locked, nothing gets past: also no swipe in from an edge.
    fn holds_edges(&self) -> bool {
        true
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if self.step != Step::Choose && self.kind == "password" && !self.asked {
            self.ask_password(cx);
        }
    }

    /// Something closed the lock screen while locked: it opens again.
    fn on_close(&mut self, cx: &mut Cx<'_, '_>) {
        if self.step == Step::Unlock && self.locked.get() {
            cx.command(Command::OpenScreen(
                SCREEN,
                Params::new().with(MODE, UNLOCK),
            ));
        }
    }

    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if self.wait_until.is_some() && !self.waiting(&env) {
            self.wait_until = None;
            self.message = None;
            self.redraw(cx);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        if *action == actions::BACK {
            if self.step != Step::Unlock {
                cx.command(Command::Back);
            } else if self.entered.pop().is_some() {
                self.redraw(cx);
            }
            return Handled::Yes;
        }
        if self.waiting(&env) {
            return Handled::Yes;
        }
        if self.step == Step::Choose {
            let kinds = Self::kinds(&env);
            let a = action;
            let up = [
                actions::UP,
                actions::LEFT,
                actions::PREVIOUS,
                actions::PREV_PAGE,
            ];
            let down = [
                actions::DOWN,
                actions::RIGHT,
                actions::NEXT,
                actions::NEXT_PAGE,
            ];
            if up.contains(a) {
                self.focus = self.focus.saturating_sub(1);
            } else if down.contains(a) {
                self.focus = (self.focus + 1).min(kinds.len() - 1);
            } else if *a == actions::CONFIRM && self.shown {
                self.choose_kind(kinds[self.focus], cx);
                return Handled::Yes;
            }
            self.shown = true;
            self.redraw(cx);
            return Handled::Yes;
        }
        let count = self.targets(&env);
        let a = action;
        let delta: isize = if *a == actions::LEFT || *a == actions::PREVIOUS {
            -1
        } else if *a == actions::RIGHT || *a == actions::NEXT {
            1
        } else if *a == actions::UP || *a == actions::PREV_PAGE {
            // A row up or down; page buttons where there are no others.
            -3
        } else if *a == actions::DOWN || *a == actions::NEXT_PAGE {
            3
        } else if *a == actions::CONFIRM {
            if self.shown {
                self.choose(cx);
            } else {
                self.shown = true;
                self.redraw(cx);
            }
            return Handled::Yes;
        } else {
            // Locked, every other action is the lock's too.
            return Handled::Yes;
        };
        if !self.shown {
            self.shown = true;
        } else {
            self.focus = (self.focus as isize + delta).clamp(0, count as isize - 1) as usize;
        }
        self.redraw(cx);
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        if gesture.kind != GestureKind::Tap || self.waiting(&env) {
            return Handled::Yes;
        }
        if let Some(action) = self.header.tap(&env, &self.style, gesture.start, None) {
            if action == actions::BACK {
                cx.command(Command::Back);
            }
            return Handled::Yes;
        }
        self.shown = false;
        if self.step == Step::Choose {
            let kinds = Self::kinds(&env);
            let hit = (0..kinds.len()).find(|&i| self.kind_row(i).contains(gesture.start));
            if let Some(i) = hit {
                self.choose_kind(kinds[i], cx);
            }
            return Handled::Yes;
        }
        match self.kind.as_str() {
            "pattern" => {}
            "password" => {
                if self.layout.password.contains(gesture.start) {
                    self.ask_password(cx);
                }
            }
            _ => {
                if let Some(key) = self
                    .layout
                    .keys
                    .iter()
                    .position(|k| k.contains(gesture.start))
                {
                    self.press(key, cx);
                }
            }
        }
        Handled::Yes
    }

    /// A finger draws a pattern from dot to dot.
    fn on_pointer(&mut self, event: &PointerEvent, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        if self.kind != "pattern" || self.step == Step::Choose || self.waiting(&env) {
            return Handled::No;
        }
        match event.phase {
            PointerPhase::Down => {
                let Some(dot) = self.layout.dot_at(event.point) else {
                    return Handled::No;
                };
                self.entered.clear();
                self.message = None;
                self.shown = false;
                self.add_dot(dot);
                self.finger = Some((event.id, event.point));
                self.redraw(cx);
                Handled::Yes
            }
            PointerPhase::Move => {
                if let Some(dot) = self.layout.dot_at(event.point) {
                    self.add_dot(dot);
                }
                let moved = self.finger.is_some_and(|(_, at)| {
                    (at.x - event.point.x).abs() + (at.y - event.point.y).abs() > 8
                });
                if moved {
                    self.finger = Some((event.id, event.point));
                    self.redraw(cx);
                }
                Handled::Yes
            }
            PointerPhase::Up => {
                let entered = core::mem::take(&mut self.entered);
                self.submit(entered, cx);
                Handled::Yes
            }
            _ => {
                self.entered.clear();
                self.finger = None;
                self.redraw(cx);
                Handled::Yes
            }
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == TYPED {
            let text = params.get(params::TEXT).unwrap_or_default().to_string();
            self.submit(text, cx);
        }
    }
}

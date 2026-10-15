//! Skimming through the book (an overlay at the bottom): where one is,
//! a bar across the book with its chapters (tap anywhere on it to go
//! there), and buttons for the chapter before and after, back to where
//! skimming began, and a page number on the keyboard. With buttons, the
//! page buttons go ten pages, left and right a chapter, confirm stays
//! and back returns to where one was.
//!
//! It asks the reader to move with the event [`crate::SKIM`] and is told
//! where the book is with [`crate::SKIMMED`].

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen,
};

/// Parameters: where the book is (thousandths), where its chapters
/// begin (thousandths, comma-separated), what to do.
pub(crate) const AT: &str = "at";
pub(crate) const MARKS: &str = "marks";
pub(crate) const TO: &str = "to";

/// The buttons: chapter back, where one was, chapter on, page number.
const BUTTONS: [&str; 4] = [
    "reader.skim.chapter-back",
    "reader.skim.return",
    "reader.skim.chapter-on",
    "reader.skim.page",
];
const MOVES: [&str; 4] = ["chapter:-1", "back", "chapter:+1", "number"];

pub(crate) struct SkimScreen {
    style: UiStyle,
    region: Rect,
    path: String,
    /// Where the book is: label, chapter and thousandths.
    label: String,
    chapter: String,
    at: u32,
    marks: Vec<u32>,
    /// The bar and the buttons.
    bar: Rect,
    buttons: [Rect; 4],
    /// The button the buttons point at (none after a touch).
    focus: Option<usize>,
}

impl SkimScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let (margin, gap, row) = (style.margin as u16, style.gap as u16, style.row as u16);
        let height = gap * 2 + row * 3;
        let region = Rect::new(
            0,
            display.height.saturating_sub(height),
            display.width,
            height,
        );
        let width = display.width.saturating_sub(2 * margin);
        let bar = Rect::new(margin, region.y + gap + row, width, row);
        let button_width = (width.saturating_sub(3 * gap)) / 4;
        let buttons_y = region.y + gap + row * 2;
        let buttons = core::array::from_fn(|i| {
            Rect::new(
                margin + i as u16 * (button_width + gap),
                buttons_y,
                button_width,
                row,
            )
        });
        let mut screen = Self {
            focus: (!env.device.input.touch).then_some(1),
            style,
            region,
            path: params.get(params::PATH).unwrap_or_default().to_string(),
            label: String::new(),
            chapter: String::new(),
            at: 0,
            marks: params
                .get(MARKS)
                .unwrap_or_default()
                .split(',')
                .filter_map(|m| m.trim().parse().ok())
                .collect(),
            bar,
            buttons,
        };
        screen.update(params);
        screen
    }

    /// Takes over where the book is now.
    fn update(&mut self, params: &Params) {
        self.label = params.get(params::PAGE).unwrap_or_default().to_string();
        self.chapter = params.get(params::TEXT).unwrap_or_default().to_string();
        self.at = params
            .get(AT)
            .and_then(|a| a.parse().ok())
            .unwrap_or(0)
            .min(1000);
    }

    fn go(&self, to: &str, cx: &mut Cx<'_, '_>) {
        cx.command(Command::Emit(
            crate::SKIM,
            Params::new()
                .with(params::PATH, self.path.clone())
                .with(TO, to.to_string()),
        ));
    }

    fn press(&self, index: usize, cx: &mut Cx<'_, '_>) {
        match MOVES[index] {
            "number" => {
                cx.command(Command::Back);
                cx.command(Command::RunAction(crate::actions::GO_TO_PAGE));
            }
            "back" => {
                self.go("back", cx);
                cx.command(Command::Back);
            }
            to => self.go(to, cx),
        }
    }

    /// The bar's track, where its line is drawn.
    fn track(&self) -> Rect {
        let style = self.style;
        let h = (style.line * 3).max(3) as u16;
        Rect::new(
            self.bar.x,
            self.bar.y + (self.bar.height - h) / 2,
            self.bar.width,
            h,
        )
    }
}

impl Screen for SkimScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        ui::fill(
            canvas,
            Rect::new(r.x, r.y, r.width, (style.line * 2) as u16),
            Color::BLACK,
        );
        let Some(font) = style.font(env) else {
            return;
        };
        // Where one is: the page, then the chapter.
        let x = f32::from(style.margin as u16);
        let info = Rect::new(
            style.margin as u16,
            r.y + style.gap as u16,
            r.width.saturating_sub(2 * style.margin as u16),
            style.row as u16,
        );
        let label_width = ui::draw_label(
            canvas,
            &font,
            style.text,
            x,
            info,
            &self.label,
            Color::BLACK,
        );
        if !self.chapter.is_empty() {
            let at = x + label_width + style.gap as f32 * 2.0;
            ui::draw_label(
                canvas,
                &font,
                style.small,
                at,
                info,
                &self.chapter,
                Color::gray(80),
            );
        }
        // The bar: read up to here, chapters as ticks, a knob where one is.
        let track = self.track();
        let width = u32::from(track.width);
        let done = (width * self.at / 1000) as u16;
        ui::fill(canvas, track, Color::gray(170));
        ui::fill(
            canvas,
            Rect::new(track.x, track.y, done, track.height),
            Color::BLACK,
        );
        let tick = track.height * 3;
        for &mark in self.marks.iter().filter(|&&m| m > 0 && m < 1000) {
            let tx = track.x + (width * mark / 1000) as u16;
            ui::fill(
                canvas,
                Rect::new(
                    tx,
                    track.y + track.height / 2 - tick / 2,
                    (style.line as u16).max(1),
                    tick,
                ),
                Color::BLACK,
            );
        }
        let knob = (self.bar.height / 2).max(8);
        let kx = (track.x + done).saturating_sub(knob / 2);
        let ky = self.bar.y + (self.bar.height - knob) / 2;
        ui::fill(canvas, Rect::new(kx, ky, knob, knob), Color::BLACK);
        // The buttons.
        for (i, (rect, key)) in self.buttons.iter().zip(BUTTONS).enumerate() {
            let h = ui::button_height(*rect);
            let button = Rect::new(rect.x, rect.y + (rect.height - h) / 2, rect.width, h);
            ui::draw_button(
                canvas,
                env,
                &style,
                button,
                env.i18n.tr(key),
                false,
                self.focus == Some(i),
            );
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        if *a == actions::PREV_PAGE || *a == actions::PREVIOUS {
            self.go("page:-10", cx);
        } else if *a == actions::NEXT_PAGE || *a == actions::NEXT {
            self.go("page:+10", cx);
        } else if *a == actions::LEFT {
            self.go("chapter:-1", cx);
        } else if *a == actions::RIGHT {
            self.go("chapter:+1", cx);
        } else if *a == actions::UP || *a == actions::DOWN {
            let len = BUTTONS.len();
            let at = self.focus.unwrap_or(0);
            self.focus = Some(if *a == actions::UP {
                (at + len - 1) % len
            } else {
                (at + 1) % len
            });
            cx.request_redraw_area(self.region);
            cx.request_refresh(RefreshMode::Fast);
        } else if *a == actions::CONFIRM {
            match self.focus {
                Some(i) if i != 1 => self.press(i, cx),
                // Stays where it is.
                _ => cx.command(Command::Back),
            }
        } else if *a == actions::BACK {
            self.go("back", cx);
            cx.command(Command::Back);
        } else if *a == actions::MENU {
            cx.command(Command::Back);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let at = gesture.start;
        if let Some(i) = self.buttons.iter().position(|b| b.contains(at)) {
            self.focus = None;
            self.press(i, cx);
        } else if self.bar.contains(at) {
            let track = self.track();
            let x = (at.x - i32::from(track.x)).clamp(0, i32::from(track.width));
            let permille = x as u32 * 1000 / u32::from(track.width.max(1));
            self.go(&alloc::format!("permille:{permille}"), cx);
        } else if !self.region.contains(at) {
            // The page: done skimming, staying here.
            cx.command(Command::Back);
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == crate::SKIMMED && params.get(params::PATH) == Some(self.path.as_str()) {
            self.update(params);
            cx.request_redraw_area(self.region);
            cx.request_refresh(RefreshMode::Fast);
        }
    }
}

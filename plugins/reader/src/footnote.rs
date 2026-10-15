//! A footnote over the page (an overlay): the note's text, *Close* and
//! *Go to the note*. Opened when a link that looks like a note mark
//! ("1", "[2]", "*") is followed and the setting `reader.footnotes` says
//! so; parameters `text`, `path` (the book) and `href` (the link).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, Screen,
};

use crate::FOLLOW;

/// The link's target, a parameter.
pub(crate) const HREF: &str = "href";
/// The most of the screen a note covers.
const SHARE: f32 = 0.5;

pub(crate) struct FootnoteScreen {
    style: UiStyle,
    region: Rect,
    lines: Vec<String>,
    /// *Close* and *Go to the note*.
    buttons: [Rect; 2],
    /// The button the buttons point at (none after a touch).
    focus: Option<usize>,
    path: String,
    href: String,
}

impl FootnoteScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let margin = style.margin as u16;
        let gap = style.gap as u16;
        let text = params.get(params::TEXT).unwrap_or_default();
        let width = f32::from(display.width.saturating_sub(2 * margin));
        let row = style.row as u16;
        let most = (f32::from(display.height) * SHARE) as u16;
        let (lines, line_height) = match style.font(env) {
            Some(font) => {
                let line = font.line_metrics(style.text).line_height as u16;
                let room = most.saturating_sub(row + 3 * gap) / line.max(1);
                let ranges = font.wrap(style.text, text, width);
                let keep = ranges.len().min(room.max(1) as usize);
                let mut lines: Vec<String> = ranges[..keep]
                    .iter()
                    .map(|r| text[r.clone()].trim_end().to_string())
                    .collect();
                if ranges.len() > keep
                    && let Some(last) = lines.last_mut()
                {
                    let start = ranges[keep - 1].start;
                    *last = ui::fit(&font, style.text, text[start..].trim(), width);
                }
                (lines, line)
            }
            None => (Vec::new(), row),
        };
        let height = gap * 3 + line_height * lines.len() as u16 + row;
        let region = Rect::new(
            0,
            display.height.saturating_sub(height),
            display.width,
            height,
        );
        let half = display.width / 2;
        let buttons_y = region.y + height - row - gap / 2;
        let buttons = [
            Rect::new(
                margin,
                buttons_y,
                half.saturating_sub(margin + gap / 2),
                row,
            ),
            Rect::new(
                half + gap / 2,
                buttons_y,
                half.saturating_sub(margin + gap / 2),
                row,
            ),
        ];
        Self {
            focus: (!env.device.input.touch).then_some(0),
            style,
            region,
            lines,
            buttons,
            path: params.get(params::PATH).unwrap_or_default().to_string(),
            href: params.get(HREF).unwrap_or_default().to_string(),
        }
    }

    fn press(&self, index: usize, cx: &mut Cx<'_, '_>) {
        cx.command(Command::Back);
        if index == 1 {
            cx.command(Command::Emit(
                FOLLOW,
                Params::new()
                    .with(params::PATH, self.path.clone())
                    .with(HREF, self.href.clone()),
            ));
        }
    }
}

impl Screen for FootnoteScreen {
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
        let line_height = font.line_metrics(style.text).line_height as u16;
        let mut y = r.y + style.gap as u16;
        for line in &self.lines {
            let area = Rect::new(r.x, y, r.width, line_height);
            let x = f32::from(r.x) + style.margin as f32;
            ui::draw_label(canvas, &font, style.text, x, area, line, Color::BLACK);
            y += line_height;
        }
        for (i, (rect, label)) in self
            .buttons
            .iter()
            .zip(["reader.footnote.close", "reader.footnote.go"])
            .enumerate()
        {
            let h = ui::button_height(*rect);
            let button = Rect::new(rect.x, rect.y + (rect.height - h) / 2, rect.width, h);
            let label = env.i18n.tr(label);
            ui::draw_button(
                canvas,
                env,
                &style,
                button,
                label,
                false,
                self.focus == Some(i),
            );
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        if [
            actions::LEFT,
            actions::UP,
            actions::PREVIOUS,
            actions::PREV_PAGE,
        ]
        .contains(a)
        {
            self.focus = Some(0);
        } else if [
            actions::RIGHT,
            actions::DOWN,
            actions::NEXT,
            actions::NEXT_PAGE,
        ]
        .contains(a)
        {
            self.focus = Some(1);
        } else if *a == actions::CONFIRM {
            self.press(self.focus.unwrap_or(0), cx);
            return Handled::Yes;
        } else if *a == actions::BACK || *a == actions::MENU {
            cx.command(Command::Back);
            return Handled::Yes;
        } else {
            return Handled::No;
        }
        cx.request_redraw();
        cx.request_refresh(quireink_sdk::RefreshMode::Fast);
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let at = gesture.start;
        if let Some(i) = self.buttons.iter().position(|b| b.contains(at)) {
            self.press(i, cx);
        } else {
            // Anywhere else: done reading the note.
            cx.command(Command::Back);
        }
        Handled::Yes
    }
}

/// Whether a link's text looks like a note mark: short, or a number in
/// brackets, or a symbol such as `*` or `†`.
pub(crate) fn note_like(label: &str) -> bool {
    let t = label.trim();
    !t.is_empty()
        && (t.chars().count() <= 3
            || t.chars().count() <= 6
                && t.chars()
                    .all(|c| c.is_ascii_digit() || "[]()*†‡§¹²³⁴⁵⁶⁷⁸⁹⁰".contains(c)))
}

#[cfg(test)]
mod tests {
    use super::note_like;

    #[test]
    fn note_marks() {
        for mark in ["1", "12", "[3]", "(14)", "*", "†", "[123]"] {
            assert!(note_like(mark), "{mark}");
        }
        for text in ["CHAPTER I.", "Notes", "", "the end"] {
            assert!(!note_like(text), "{text}");
        }
    }
}

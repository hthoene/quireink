//! Short messages ([`slots::NOTICE`](quireink_sdk::builtin::slots::NOTICE)):
//! a framed box above the bottom of the screen, between the margins, with
//! the text centred in at most three lines. The kernel shows it over
//! whatever is open and takes it away after a few seconds.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::params;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{Canvas, Color, Cx, Env, Handled, Id, Params, Rect, Screen};

/// Lines of a message, at most.
const LINES: usize = 3;

pub(crate) struct NoticeScreen {
    style: UiStyle,
    region: Rect,
    lines: Vec<String>,
}

impl NoticeScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let margin = style.margin as u16;
        let pad = style.gap as u16;
        let width = display.width.saturating_sub(2 * margin);
        let inner = f32::from(width.saturating_sub(2 * pad));
        let text = params.get(params::TEXT).unwrap_or_default();
        let (lines, line_height) = match style.font(env) {
            Some(font) => {
                let ranges = font.wrap(style.text, text, inner);
                let mut lines: Vec<String> = ranges
                    .iter()
                    .take(LINES)
                    .map(|r| text[r.clone()].trim().to_string())
                    .collect();
                if ranges.len() > LINES
                    && let (Some(last), Some(range)) = (lines.last_mut(), ranges.get(LINES - 1))
                {
                    *last = ui::fit(&font, style.text, text[range.start..].trim(), inner);
                }
                (lines, font.line_metrics(style.text).line_height as u16)
            }
            None => (Vec::new(), style.row as u16),
        };
        let height = line_height * lines.len().max(1) as u16 + 2 * pad;
        // Above the reader's status line, out of the way of the page.
        let bottom = display.height.saturating_sub(style.row as u16);
        let region = Rect::new(margin, bottom.saturating_sub(height), width, height);
        Self {
            style,
            region,
            lines,
        }
    }
}

impl Screen for NoticeScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        canvas.stroke_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            u32::from(r.height),
            style.line * 2,
            Color::BLACK,
        );
        let Some(font) = style.font(env) else {
            return;
        };
        let line_height = font.line_metrics(style.text).line_height as u16;
        let mut y = r.y + style.gap as u16;
        for line in &self.lines {
            let width = font.measure(style.text, line);
            let x = f32::from(r.x) + (f32::from(r.width) - width) / 2.0;
            let area = Rect::new(r.x, y, r.width, line_height);
            ui::draw_label(canvas, &font, style.text, x, area, line, Color::BLACK);
            y += line_height;
        }
    }

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        Handled::No
    }
}

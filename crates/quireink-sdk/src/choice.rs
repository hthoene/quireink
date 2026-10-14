//! An overlay at the bottom with a title and a few choices, e.g. what to
//! do with a file or whether to delete it.
//!
//! A plugin registers it as a screen of its own and opens it with
//! [`open`]; the choice comes back as the plugin's reply event, with the
//! context it was opened for ([`params::PATH`]) and the [`OPTION`]
//! chosen. Tapping outside, or *Back*, chooses nothing.
//!
//! ```ignore
//! r.add_screen(ScreenSpec {
//!     id: CHOICE,
//!     title: Cow::Borrowed("my.title"),
//!     factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
//! })?;
//! cx.command(choice::open(CHOICE, CHOSEN, "Delete “a.epub”?", "books/a.epub", "my.option.", &["delete", "cancel"]));
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::builtin::{actions, params};
use crate::ui::{self, ListChange, ListView, UiStyle};
use crate::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen,
};

/// The option chosen.
pub const OPTION: &str = "option";
const OPTIONS: &str = "options";
/// Translation keys of the options: this, then the option.
const LABELS: &str = "labels";
/// Lines of the title, at most.
const TITLE_LINES: usize = 3;

/// Opens the choice overlay `screen` with `title` and `options`, each
/// shown as the translation of `labels` followed by the option (e.g.
/// `file-manager.option.` and `delete`). The choice comes back as the
/// event `reply` with `context` as [`params::PATH`].
pub fn open(
    screen: Id,
    reply: Id,
    title: &str,
    context: &str,
    labels: &str,
    options: &[&str],
) -> Command {
    Command::OpenScreen(
        screen,
        Params::new()
            .with(params::TITLE, title.to_string())
            .with(params::PATH, context.to_string())
            .with(params::REPLY, reply.as_str().to_string())
            .with(LABELS, labels.to_string())
            .with(OPTIONS, options.join(",")),
    )
}

/// The choice overlay; see the [module documentation](self).
#[derive(Debug)]
pub struct Choice {
    style: UiStyle,
    region: Rect,
    title: Rect,
    list: ListView,
    title_text: String,
    path: String,
    reply: Option<Id>,
    labels: String,
    options: Vec<String>,
}

impl Choice {
    pub fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let options: Vec<String> = params
            .get(OPTIONS)
            .unwrap_or_default()
            .split(',')
            .filter(|o| !o.is_empty())
            .map(ToString::to_string)
            .collect();
        let title_text = params.get(params::TITLE).unwrap_or_default().to_string();
        let row = style.row as u16;
        // A long title takes up to three lines.
        let lines = style.font(env).map_or((1, 0.0), |font| {
            let width = f32::from(display.width) - 2.0 * style.margin as f32;
            let line = font.line_metrics(style.title).line_height;
            let count = font
                .wrap(style.title, &title_text, width)
                .len()
                .clamp(1, TITLE_LINES);
            (count, line)
        });
        let title_height = match lines {
            (1, _) => row,
            (count, line) => (count as f32 * line) as u16 + style.gap as u16,
        };
        let height =
            (title_height + row * options.len() as u16 + style.gap as u16 * 2).min(display.height);
        let region = Rect::new(0, display.height - height, display.width, height);
        let title = Rect::new(0, region.y + style.gap as u16, display.width, title_height);
        let list = ListView::new(
            Rect::new(
                0,
                title.y + title_height,
                display.width,
                row * options.len() as u16,
            ),
            style.row,
        );
        Self {
            style,
            region,
            title,
            list,
            title_text,
            path: params.get(params::PATH).unwrap_or_default().to_string(),
            reply: params
                .get(params::REPLY)
                .and_then(|reply| Id::parse(reply).ok()),
            labels: params.get(LABELS).unwrap_or_default().to_string(),
            options,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        cx.command(Command::Back);
        if let (Some(option), Some(reply)) = (self.options.get(index), &self.reply) {
            cx.command(Command::Emit(
                reply.clone(),
                Params::new()
                    .with(params::PATH, self.path.clone())
                    .with(OPTION, option.clone()),
            ));
        }
    }
}

impl Screen for Choice {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            (style.line * 2).max(2),
            Color::BLACK,
        );
        if let Some(font) = style.font(env) {
            let margin = style.margin as f32;
            let width = f32::from(self.title.width) - 2.0 * margin;
            let mut lines = font.wrap(style.title, &self.title_text, width);
            if lines.len() <= 1 {
                ui::draw_label(
                    canvas,
                    &font,
                    style.title,
                    margin,
                    self.title,
                    &self.title_text,
                    Color::BLACK,
                );
            } else {
                let more = lines.len() > TITLE_LINES;
                lines.truncate(TITLE_LINES);
                let line = font.line_metrics(style.title).line_height;
                let count = lines.len();
                for (n, range) in lines.into_iter().enumerate() {
                    let text = if more && n + 1 == count {
                        // The rest, shortened.
                        alloc::borrow::Cow::Owned(ui::fit(
                            &font,
                            style.title,
                            &self.title_text[range.start..],
                            width,
                        ))
                    } else {
                        alloc::borrow::Cow::Borrowed(&self.title_text[range])
                    };
                    let y = self.title.y + (n as f32 * line) as u16;
                    let area = Rect::new(0, y, self.title.width, line as u16);
                    ui::draw_label(
                        canvas,
                        &font,
                        style.title,
                        margin,
                        area,
                        &text,
                        Color::BLACK,
                    );
                }
            }
        }
        let (options, labels) = (&self.options, &self.labels);
        self.list.draw(
            canvas,
            &style,
            options.len(),
            |canvas, index, rect, focused| {
                let key = alloc::format!("{labels}{}", options[index]);
                ui::draw_value_row(canvas, env, &style, rect, focused, env.i18n.tr(&key), "");
            },
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let delta = if [
            actions::UP,
            actions::LEFT,
            actions::PREVIOUS,
            actions::PREV_PAGE,
        ]
        .contains(a)
        {
            -1
        } else if [
            actions::DOWN,
            actions::RIGHT,
            actions::NEXT,
            actions::NEXT_PAGE,
        ]
        .contains(a)
        {
            1
        } else if *a == actions::CONFIRM {
            self.choose(self.list.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.list.move_focus(delta, self.options.len()) != ListChange::None {
            cx.request_redraw_area(self.list.area);
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(index) = self.list.hit(gesture.start, self.options.len()) {
            self.choose(index, cx);
        }
        Handled::Yes
    }
}

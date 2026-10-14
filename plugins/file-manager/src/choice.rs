//! An overlay at the bottom with a title and a few choices, e.g. what to
//! do with a file or whether to delete it. The choice comes back as the
//! event `file-manager.chosen` with the path and [`OPTION`].

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen,
};

use crate::CHOSEN;

/// The option chosen.
pub(crate) const OPTION: &str = "option";
const OPTIONS: &str = "options";

/// Opens the overlay `screen` for `path` with `title` and `options` (each
/// shown as `file-manager.option.<option>`).
pub(crate) fn open(screen: Id, title: &str, path: &str, options: &[&str]) -> Command {
    Command::OpenScreen(
        screen,
        Params::new()
            .with(params::TITLE, title.to_string())
            .with(params::PATH, path.to_string())
            .with(OPTIONS, options.join(",")),
    )
}

pub(crate) struct Choice {
    style: UiStyle,
    region: Rect,
    title: Rect,
    list: ListView,
    title_text: String,
    path: String,
    options: Vec<String>,
}

impl Choice {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let options: Vec<String> = params
            .get(OPTIONS)
            .unwrap_or_default()
            .split(',')
            .filter(|o| !o.is_empty())
            .map(ToString::to_string)
            .collect();
        let row = style.row as u16;
        let height = (row * (options.len() as u16 + 1) + style.gap as u16 * 2).min(display.height);
        let region = Rect::new(0, display.height - height, display.width, height);
        let title = Rect::new(0, region.y + style.gap as u16, display.width, row);
        let list = ListView::new(
            Rect::new(0, title.y + row, display.width, row * options.len() as u16),
            style.row,
        );
        Self {
            style,
            region,
            title,
            list,
            title_text: params.get(params::TITLE).unwrap_or_default().to_string(),
            path: params.get(params::PATH).unwrap_or_default().to_string(),
            options,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        cx.command(Command::Back);
        if let Some(option) = self.options.get(index) {
            cx.command(Command::Emit(
                CHOSEN,
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
            ui::draw_label(
                canvas,
                &font,
                style.title,
                margin,
                self.title,
                &self.title_text,
                Color::BLACK,
            );
        }
        let options = &self.options;
        self.list.draw(
            canvas,
            &style,
            options.len(),
            |canvas, index, rect, focused| {
                let key = alloc::format!("file-manager.option.{}", options[index]);
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

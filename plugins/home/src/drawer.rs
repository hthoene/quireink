//! Every tool at once, as icons in a grid (as a phone lists its apps):
//! opened by swiping up on the home screen. Tapping one runs it; the
//! buttons move between them. And the hint on the gestures, once, the
//! first time the home screen shows on a touch screen.

use alloc::vec::Vec;

use quireink_sdk::builtin::actions;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Rect, RefreshMode, Screen,
};

use crate::{blocks, grid};

/// Columns of the overview.
const COLUMNS: u32 = 4;

pub(crate) struct DrawerScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    tools: Vec<Id>,
}

impl DrawerScreen {
    pub(crate) fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(
            style.margin as u16,
            top,
            display.width.saturating_sub(2 * style.margin as u16),
            display.height.saturating_sub(top),
        );
        // About as high as a column is wide.
        let row = (u32::from(area.width) / COLUMNS * 4 / 5).max(style.row);
        Self {
            list: ListView::new(area, row).columns(COLUMNS).focus_on_keys(env),
            tools: grid::tools(env).into_iter().map(|(id, _)| id).collect(),
            style,
            header,
        }
    }

    fn run(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if let Some(action) = self.tools.get(index) {
            cx.command(Command::Back);
            cx.command(Command::RunAction(action.clone()));
        }
    }
}

impl Screen for DrawerScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.tools.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("home.drawer"),
            None,
            self.list.paging(len),
        );
        let tools = &self.tools;
        let list = &self.list;
        let first = list.page() * list.rows();
        for (slot, index) in (first..len).take(list.rows()).enumerate() {
            let rect = list.row_rect(slot);
            let focused = list.shown && index == list.focus;
            canvas.describe(
                quireink_sdk::semantics::Role::Button,
                "",
                rect,
                focused,
                None,
            );
            blocks::draw_tool(canvas, env, rect, &tools[index]);
            if focused {
                canvas.stroke_rect(
                    i32::from(rect.x),
                    i32::from(rect.y),
                    u32::from(rect.width),
                    u32::from(rect.height),
                    style.line * 3,
                    Color::BLACK,
                );
            }
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.tools.len();
        let a = action;
        let columns = COLUMNS as isize;
        let delta = if *a == actions::LEFT || *a == actions::PREVIOUS {
            -1
        } else if *a == actions::RIGHT || *a == actions::NEXT {
            1
        } else if *a == actions::UP {
            -columns
        } else if *a == actions::DOWN {
            columns
        } else if *a == actions::NEXT_PAGE {
            self.list.rows() as isize
        } else if *a == actions::PREV_PAGE {
            -(self.list.rows() as isize)
        } else if *a == actions::CONFIRM {
            self.run(self.list.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.list.move_focus(delta, len) != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        match gesture.kind {
            GestureKind::Tap => {
                let len = self.tools.len();
                let paging = self.list.paging(len);
                if let Some(action) = self
                    .header
                    .tap(cx.env(), &self.style, gesture.start, paging)
                {
                    cx.command(Command::RunAction(action));
                    return Handled::Yes;
                }
                match self.list.hit(gesture.start, len) {
                    Some(index) => {
                        self.run(index, cx);
                        Handled::Yes
                    }
                    None => Handled::No,
                }
            }
            // Down again: back to the home screen.
            GestureKind::Swipe(quireink_sdk::Direction::Down) => {
                cx.command(Command::Back);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

/// Whether the hint on gestures shows (once; switched off when shown).
pub(crate) const TIPS_SETTING: Id = Id::new_static("home.gesture-tips");

/// Whether the hint on gestures is due: on touch screens, until shown.
pub(crate) fn tips_due(env: &Env<'_>) -> bool {
    env.device.input.touch && env.settings.bool(TIPS_SETTING.as_str())
}

/// The hint on the gestures, in a box in the middle of the screen: what
/// the edges do, and that a long press arranges the home screen.
pub(crate) struct TipsScreen {
    style: UiStyle,
    region: Rect,
    ok: Rect,
}

/// The lines of the hint, in order.
const TIPS: [&str; 5] = [
    "home.tips.home",
    "home.tips.tools",
    "home.tips.quick",
    "home.tips.back",
    "home.tips.arrange",
];

impl TipsScreen {
    pub(crate) fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let line = style.row as u16;
        let width = display.width.saturating_sub(4 * style.margin as u16);
        let height = style.row as u16 * 2 + line * TIPS.len() as u16 + style.gap as u16 * 2;
        let region = Rect::new(
            (display.width - width) / 2,
            display.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let button_w = width / 3;
        let ok = Rect::new(
            region.x + (width - button_w) / 2,
            region.y + height - style.row as u16 - style.gap as u16 / 2,
            button_w,
            ui::button_height(Rect::new(0, 0, 0, style.row as u16)),
        );
        Self { style, region, ok }
    }
}

impl Screen for TipsScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    /// Shown once: switched off as it shows.
    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        cx.command(Command::SetSetting(
            TIPS_SETTING,
            quireink_sdk::SettingValue::Bool(false),
        ));
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        ui::stroke_round(canvas, &style, r, style.line * 2, Color::BLACK);
        let Some(font) = style.font(env) else {
            return;
        };
        let margin = style.margin as u16;
        let title = Rect::new(
            r.x + margin,
            r.y + style.gap as u16,
            r.width - 2 * margin,
            style.row as u16,
        );
        // Centred, as the lines below.
        let text = env.i18n.tr("home.tips.title");
        let x =
            f32::from(title.x) + (f32::from(title.width) - font.measure(style.title, text)) / 2.0;
        ui::draw_label(canvas, &font, style.title, x, title, text, Color::BLACK);
        let line = style.row as u16;
        for (n, key) in TIPS.iter().enumerate() {
            let row = Rect::new(r.x, title.y + title.height + n as u16 * line, r.width, line);
            ui::draw_message(canvas, env, &style, row, env.i18n.tr(key));
        }
        let ok = env.i18n.tr("home.tips.ok");
        ui::draw_button(canvas, env, &style, self.ok, ok, false, true);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if *action == actions::CONFIRM || *action == actions::BACK {
            cx.command(Command::Back);
            return Handled::Yes;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind == GestureKind::Tap {
            cx.command(Command::Back);
        }
        Handled::Yes
    }
}

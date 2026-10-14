//! The editor of the setting `keyboard.layouts`: every layout with *On*
//! or *Off*; the order of switching on is the order of switching layouts.
//! *Automatic* (the UI language's layout) stands for itself: choosing it
//! leaves the others, choosing another layout leaves it.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use quireink_sdk::builtin::actions;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Rect, RefreshMode, Screen,
    SettingValue,
};

use crate::LAYOUTS_SETTING;
use crate::layouts::{AUTO, LAYOUTS};

pub(crate) struct ChooseScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
}

/// The rows: *Automatic*, then the built-in layouts.
fn ids() -> impl Iterator<Item = &'static str> {
    core::iter::once(AUTO).chain(LAYOUTS.iter().map(|l| l.id))
}

fn chosen<'e>(env: &Env<'e>) -> Vec<&'e str> {
    env.settings
        .text(LAYOUTS_SETTING.as_str())
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect()
}

impl ChooseScreen {
    pub(crate) fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height - top);
        Self {
            list: ListView::new(area, style.row),
            style,
            header,
        }
    }

    fn toggle(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(id) = ids().nth(index) else {
            return;
        };
        let mut chosen = chosen(&env);
        if id == AUTO {
            chosen = Vec::from([AUTO]);
        } else {
            chosen.retain(|c| *c != AUTO);
            match chosen.iter().position(|c| *c == id) {
                Some(at) => {
                    chosen.remove(at);
                }
                None => chosen.push(id),
            }
            if chosen.is_empty() {
                chosen.push(AUTO);
            }
        }
        cx.command(Command::SetSetting(
            LAYOUTS_SETTING,
            SettingValue::Text(Cow::Owned(chosen.join(", "))),
        ));
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}

impl Screen for ChooseScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = ids().count();
        let title = env.i18n.tr("keyboard.setting.layouts");
        self.header
            .draw(canvas, env, &style, title, None, self.list.paging(len));
        let chosen = chosen(env);
        let rows: Vec<&str> = ids().collect();
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let id = rows[index];
                let on = chosen.iter().position(|c| *c == id);
                // Chosen layouts carry their place in switching: "On (2)".
                let value = match on {
                    Some(at) if chosen.len() > 1 => {
                        format!("{} ({})", env.i18n.tr("settings.on"), at + 1)
                    }
                    Some(_) => env.i18n.tr("settings.on").to_string(),
                    None => env.i18n.tr("settings.off").to_string(),
                };
                ui::draw_value_row(canvas, env, &style, rect, focused, env.i18n.tr(id), &value);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = ids().count();
        let rows = self.list.rows() as isize;
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            rows
        } else if *a == actions::PREV_PAGE {
            -rows
        } else if *a == actions::CONFIRM {
            self.toggle(self.list.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        let change = self.list.move_focus(delta, len);
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let len = ids().count();
        let paging = self.list.paging(len);
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, len) else {
            return Handled::No;
        };
        self.list.focus = index;
        self.toggle(index, cx);
        Handled::Yes
    }
}

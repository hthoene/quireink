//! Choosing what a status line shows: a screen for any setting that lists
//! status items (or status lines), bound by the kernel as
//! [`screens::STATUS_ITEMS`](crate::builtin::screens::STATUS_ITEMS) and
//! [`screens::STATUS_LINES`](crate::builtin::screens::STATUS_LINES). Name
//! one as the setting's editor ([`SettingSpec::editor`](crate::SettingSpec)):
//! it lists the items shown, in their order, then the others; tapping one
//! shows or hides it (a newly shown one goes last), and shown items are
//! moved by dragging their grip.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::builtin::{actions, params};
use crate::status::item_ids;
use crate::ui::{self, ListChange, ListView, Reorder, ReorderStep, UiStyle};
use crate::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, PointerEvent, Rect,
    RefreshMode, Screen, SettingValue,
};

/// The value of [`params::MODE`] that lists status lines, not items.
pub const LINES: &str = "lines";

/// The screen; see the [module documentation](self).
pub struct ItemsScreen {
    setting: Option<Id>,
    /// Item ids and the translation keys of their names.
    items: Vec<(Id, String)>,
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    reorder: Reorder,
}

impl ItemsScreen {
    /// For the setting [`params::ID`]; status lines with [`params::MODE`]
    /// [`LINES`], else status items.
    pub fn open(env: &Env<'_>, params: &Params) -> Self {
        let setting = params.get(params::ID).and_then(|id| Id::parse(id).ok());
        let items = if params.get(params::MODE) == Some(LINES) {
            env.status_items
                .lines()
                .map(|(id, title)| (id.clone(), title.to_string()))
                .collect()
        } else {
            env.status_items
                .iter()
                .map(|(id, title)| (id.clone(), title.to_string()))
                .collect()
        };
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height - top);
        Self {
            setting,
            items,
            list: ListView::new(area, style.row).focus_on_keys(env),
            reorder: Reorder::default(),
            style,
            header,
        }
    }

    /// The items shown, in their order (only those there are).
    fn chosen<'e>(&self, env: &Env<'e>) -> Vec<&'e str> {
        self.setting
            .as_ref()
            .map(|s| {
                item_ids(env.settings.text(s.as_str()))
                    .filter(|c| self.items.iter().any(|(id, _)| id.as_str() == *c))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The rows: indexes into the items, the shown ones first.
    fn order(&self, env: &Env<'_>) -> Vec<usize> {
        let chosen = self.chosen(env);
        let at = |c: &str| self.items.iter().position(|(id, _)| id.as_str() == c);
        let mut order: Vec<usize> = chosen.iter().filter_map(|c| at(c)).collect();
        let rest: Vec<usize> = (0..self.items.len())
            .filter(|i| !order.contains(i))
            .collect();
        order.extend(rest);
        order
    }

    fn keep(&self, chosen: &[&str], cx: &mut Cx<'_, '_>) {
        if let Some(setting) = &self.setting {
            cx.command(Command::SetSetting(
                setting.clone(),
                SettingValue::Text(Cow::Owned(chosen.join(", "))),
            ));
        }
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn toggle(&mut self, row: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some((id, _)) = self.order(&env).get(row).and_then(|&i| self.items.get(i)) else {
            return;
        };
        let mut chosen = self.chosen(&env);
        match chosen.iter().position(|c| *c == id.as_str()) {
            Some(at) => {
                chosen.remove(at);
            }
            None => chosen.push(id.as_str()),
        }
        self.keep(&chosen, cx);
    }
}

impl core::fmt::Debug for ItemsScreen {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ItemsScreen")
            .field("setting", &self.setting)
            .finish_non_exhaustive()
    }
}

impl Screen for ItemsScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let title = self.setting.as_ref().map_or(String::new(), |s| {
            env.settings
                .specs()
                .find(|spec| spec.id == *s)
                .map_or(String::new(), |spec| env.i18n.tr(&spec.title).to_string())
        });
        let len = self.items.len();
        self.header
            .draw(canvas, env, &style, &title, None, self.list.paging(len));
        let chosen = self.chosen(env);
        let order = self.order(env);
        let items = &self.items;
        self.list
            .draw(canvas, &style, len, |canvas, row, rect, focused| {
                let (id, name) = &items[order[row]];
                let on = chosen.iter().position(|c| *c == id.as_str());
                let rect = if on.is_some() {
                    Reorder::draw_grip(canvas, env, &style, rect, focused);
                    Reorder::content(rect)
                } else {
                    rect
                };
                // Shown items carry their place: "On (2)".
                let value = match on {
                    Some(at) if chosen.len() > 1 => {
                        format!("{} ({})", env.i18n.tr("core.state.on"), at + 1)
                    }
                    Some(_) => env.i18n.tr("core.state.on").to_string(),
                    None => env.i18n.tr("core.state.off").to_string(),
                };
                ui::draw_value_row(
                    canvas,
                    env,
                    &style,
                    rect,
                    focused,
                    env.i18n.tr(name),
                    &value,
                );
            });
        self.reorder.draw(canvas, &style, &self.list);
    }

    fn on_pointer(&mut self, event: &PointerEvent, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let mut chosen = self.chosen(&env);
        let Some(step) = self.reorder.on_pointer(event, &self.list, chosen.len()) else {
            return Handled::No;
        };
        match step {
            ReorderStep::Dropped { from, to } if from != to => {
                ui::reorder::moved(&mut chosen, from, to);
                self.list.focus = to;
                self.keep(&chosen, cx);
            }
            ReorderStep::Same => {}
            _ => {
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            }
        }
        Handled::Yes
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.items.len();
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
        if self.list.move_focus(delta, len) != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = self.list.paging(self.items.len());
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging);
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.items.len()) {
            Some(index) => {
                self.list.focus = index;
                self.toggle(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}

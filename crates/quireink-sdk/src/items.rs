//! Choosing what a status line shows: a screen for any setting that lists
//! status items (or status lines), bound by the kernel as
//! [`screens::STATUS_ITEMS`](crate::builtin::screens::STATUS_ITEMS) and
//! [`screens::STATUS_LINES`](crate::builtin::screens::STATUS_LINES). Name
//! one as the setting's editor ([`SettingSpec::editor`](crate::SettingSpec)):
//! it lists every item with whether it is shown, and tapping one shows or
//! hides it (shown items keep the order in which they were chosen).

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::builtin::{actions, params};
use crate::status::item_ids;
use crate::ui::{self, ListChange, ListView, UiStyle};
use crate::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode, Screen,
    SettingValue,
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
            style,
            header,
        }
    }

    fn chosen<'e>(&self, env: &Env<'e>) -> Vec<&'e str> {
        self.setting
            .as_ref()
            .map(|s| item_ids(env.settings.text(s.as_str())).collect())
            .unwrap_or_default()
    }

    fn toggle(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let (Some(setting), Some((id, _))) = (&self.setting, self.items.get(index)) else {
            return;
        };
        let mut chosen = self.chosen(&env);
        match chosen.iter().position(|c| *c == id.as_str()) {
            Some(at) => {
                chosen.remove(at);
            }
            None => chosen.push(id.as_str()),
        }
        cx.command(Command::SetSetting(
            setting.clone(),
            SettingValue::Text(Cow::Owned(chosen.join(", "))),
        ));
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
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
        let items = &self.items;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let (id, name) = &items[index];
                let on = chosen.iter().position(|c| *c == id.as_str());
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

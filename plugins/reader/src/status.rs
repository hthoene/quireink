//! The reader's status lines above and below the text.
//!
//! Each line has a left, centre and right part; a setting per part lists
//! the status items shown there (see `quireink_sdk::status`), chosen on
//! [`ItemsScreen`]. Items come from any plugin: the kernel's time and
//! battery, the reader's title, chapter, page and progress, and whatever
//! other plugins add. A line without items takes no space.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::status::{StatusLine, StatusValue, context, item_ids};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    RegisterError, Registrar, Screen, ScreenSpec, SettingGroupSpec, SettingKind, SettingSpec,
    SettingValue, StatusItemSpec,
};

/// The settings group of the status lines.
pub const GROUP: Id = Id::new_static("reader.group.status");
/// The screen that chooses the items of one part (parameter `id`: the
/// part's setting).
pub const ITEMS_SCREEN: Id = Id::new_static("reader.status-items");

/// The parts: top left, centre, right, then bottom left, centre, right.
pub const PARTS: [Id; 6] = [
    Id::new_static("reader.status.top-left"),
    Id::new_static("reader.status.top-center"),
    Id::new_static("reader.status.top-right"),
    Id::new_static("reader.status.bottom-left"),
    Id::new_static("reader.status.bottom-center"),
    Id::new_static("reader.status.bottom-right"),
];

/// The reader's status items.
pub mod items {
    use quireink_sdk::Id;

    pub const TITLE: Id = Id::new_static("reader.status.title");
    /// The chapter, or the title in books without a table of contents.
    pub const CHAPTER: Id = Id::new_static("reader.status.chapter");
    /// `12 / 340`, or a percentage while pages are being counted.
    pub const PAGE: Id = Id::new_static("reader.status.page");
    /// Reading progress in percent.
    pub const PROGRESS: Id = Id::new_static("reader.status.progress");
}

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    r.add_setting_group(SettingGroupSpec {
        id: GROUP,
        title: Cow::Borrowed("reader.group.status"),
        order: 15,
    })?;
    // The chapter, the time and the battery with the page.
    let defaults = [
        "",
        "",
        "",
        "reader.status.chapter",
        "core.status.clock",
        "core.status.battery, reader.status.page",
    ];
    let titles = [
        "reader.status.top-left",
        "reader.status.top-center",
        "reader.status.top-right",
        "reader.status.bottom-left",
        "reader.status.bottom-center",
        "reader.status.bottom-right",
    ];
    for ((id, title), default) in PARTS.into_iter().zip(titles).zip(defaults) {
        let spec = SettingSpec::new(
            id,
            title,
            SettingKind::List,
            SettingValue::Text(Cow::Owned(default.to_string())),
        )
        .group(GROUP)
        .editor(ITEMS_SCREEN);
        r.add_setting(spec)?;
    }
    r.add_screen(ScreenSpec {
        id: ITEMS_SCREEN,
        title: Cow::Borrowed("reader.group.status"),
        factory: Box::new(|env, params| Box::new(ItemsScreen::open(env, params))),
    })?;
    let text = |value: Option<&str>| value.map(|v| StatusValue::Text(v.to_string()));
    r.add_status_item(StatusItemSpec {
        id: items::TITLE,
        title: Cow::Borrowed("reader.status.item.title"),
        value: Box::new(move |_, cx| text(cx.get(params::TITLE))),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::CHAPTER,
        title: Cow::Borrowed("reader.status.item.chapter"),
        value: Box::new(move |_, cx| text(cx.get(context::CHAPTER).or(cx.get(params::TITLE)))),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::PAGE,
        title: Cow::Borrowed("reader.status.item.page"),
        value: Box::new(move |_, cx| text(cx.get(context::PAGE))),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::PROGRESS,
        title: Cow::Borrowed("reader.status.item.progress"),
        value: Box::new(|env, cx| {
            let percent: f32 = cx.get(params::PROGRESS)?.parse().ok()?;
            Some(StatusValue::Text(env.i18n.format(
                "reader.percent",
                &[("percent", Arg::Int(percent as i64))],
            )))
        }),
    })?;
    Ok(())
}

/// The item lists of a line's parts: `top` or bottom.
pub(crate) fn lists<'e>(env: &Env<'e>, top: bool) -> [&'e str; 3] {
    let offset = if top { 0 } else { 3 };
    core::array::from_fn(|i| env.settings.text(PARTS[offset + i].as_str()))
}

/// The status line in `area`; text size and colour as the reader's.
pub(crate) fn line<'e>(env: &Env<'e>, area: Rect, top: bool) -> StatusLine<'e> {
    StatusLine {
        area,
        lists: lists(env, top),
        size: env.metrics.pt(8.0),
        color: quireink_sdk::Color::gray(80),
    }
}

/// Whether any part shows item `id`.
pub(crate) fn uses(env: &Env<'_>, id: &Id) -> bool {
    PARTS
        .iter()
        .any(|part| item_ids(env.settings.text(part.as_str())).any(|i| i == id.as_str()))
}

/// Whether the line `top` (or bottom) shows anything.
pub(crate) fn shown(env: &Env<'_>, top: bool) -> bool {
    lists(env, top)
        .iter()
        .any(|list| item_ids(list).next().is_some())
}

/// What all status lines show now, to notice when they need a redraw.
pub(crate) fn signature(env: &Env<'_>, context: &Params) -> String {
    let mut out = String::new();
    for list in lists(env, true).iter().chain(&lists(env, false)) {
        for id in item_ids(list) {
            if let Some(value) = env.status_items.value(id, env, context) {
                out.push_str(&format!("{value:?}|"));
            }
        }
    }
    out
}

/// Chooses the items of one part of a status line: every status item with
/// whether it is shown; tapping one shows or hides it (shown items keep
/// the order in which they were chosen).
pub(crate) struct ItemsScreen {
    setting: Option<Id>,
    /// Item ids and the translation keys of their names.
    items: Vec<(Id, String)>,
    style: UiStyle,
    header: ui::Header,
    list: ListView,
}

impl ItemsScreen {
    fn open(env: &Env<'_>, params: &Params) -> Self {
        let setting = params
            .get(params::ID)
            .and_then(|id| Id::parse(id).ok())
            .filter(|id| PARTS.contains(id));
        let items = env
            .status_items
            .iter()
            .map(|(id, title)| (id.clone(), title.to_string()))
            .collect();
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height - top);
        Self {
            setting,
            items,
            list: ListView::new(area, style.row),
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
                        format!("{} ({})", env.i18n.tr("settings.on"), at + 1)
                    }
                    Some(_) => env.i18n.tr("settings.on").to_string(),
                    None => env.i18n.tr("settings.off").to_string(),
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

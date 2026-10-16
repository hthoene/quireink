//! Editing a list of widgets, as a list that buttons work as well as
//! taps: the widgets (choosing one offers to move it up or down, its own
//! settings where it has some, or to remove it), then *Add a widget*; for a page of the home screen also
//! *Add a page* and *Remove this page*. Every change is kept at once.
//!
//! It edits a page of the home screen (parameter `page`, or `id` with the
//! page's setting) or any other setting that is a list of widgets
//! (parameter `id`), e.g. the sleep screen's: such settings name this
//! screen as their editor.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, params, slots};
use quireink_sdk::status::item_ids;
use quireink_sdk::ui::choice;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::widgets;
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, SettingValue,
};

use crate::pages::{self, MAX_PAGES, STATUS};
use crate::{ADD_SCREEN, CHOICE};

/// A widget's options were chosen.
const CHOSEN: Id = Id::new_static("home.edit-chosen");
/// Parameter: the page edited (0 the first).
pub(crate) const PAGE: &str = "page";
const OPTION_LABELS: &str = "home.edit.";

/// What is edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A page of the home screen (0 the first).
    Page(usize),
    /// Another setting that lists widgets.
    Setting(String),
}

/// What the parameters ask to edit.
pub(crate) fn target_of(params: &Params) -> Target {
    if let Some(page) = params.get(PAGE).and_then(|p| p.parse().ok()) {
        return Target::Page(page);
    }
    let Some(id) = params.get(params::ID) else {
        return Target::Page(0);
    };
    match (0..MAX_PAGES).find(|&i| pages::setting(i).as_str() == id) {
        Some(page) => Target::Page(page),
        None => Target::Setting(id.to_string()),
    }
}

impl Target {
    /// Its widgets.
    fn widgets(&self, env: &Env<'_>) -> Vec<String> {
        match self {
            Target::Page(page) => pages::pages(env).get(*page).cloned().unwrap_or_default(),
            Target::Setting(id) => item_ids(env.settings.text(id))
                .map(ToString::to_string)
                .collect(),
        }
    }

    /// The commands that keep `widgets` as its list.
    fn store(&self, env: &Env<'_>, widgets: Vec<String>) -> Vec<Command> {
        match self {
            Target::Page(page) => {
                let mut pages = pages::pages(env);
                if let Some(list) = pages.get_mut(*page) {
                    *list = widgets;
                }
                pages::store(&pages)
            }
            Target::Setting(id) => Id::parse(id)
                .ok()
                .map(|id| {
                    Command::SetSetting(id, SettingValue::Text(Cow::Owned(widgets.join(", "))))
                })
                .into_iter()
                .collect(),
        }
    }

    /// The parameters that ask to edit it.
    fn params(&self) -> Params {
        match self {
            Target::Page(page) => Params::new().with(PAGE, format!("{page}")),
            Target::Setting(id) => Params::new().with(params::ID, id.clone()),
        }
    }

    fn title(&self, env: &Env<'_>) -> String {
        match self {
            Target::Page(page) => env
                .i18n
                .format("home.edit.title", &[("page", Arg::Int(*page as i64 + 1))]),
            Target::Setting(id) => env
                .settings
                .specs()
                .find(|s| s.id.as_str() == id)
                .map_or(id.clone(), |s| env.i18n.tr(&s.title).to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Widget(usize),
    Add,
    AddPage,
    RemovePage,
}

pub(crate) struct EditScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    target: Target,
}

impl EditScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
            target: target_of(params),
        }
    }

    fn rows(&self, env: &Env<'_>) -> Vec<Row> {
        let count = self.target.widgets(env).len();
        let mut rows: Vec<Row> = (0..count).map(Row::Widget).collect();
        rows.push(Row::Add);
        if let Target::Page(_) = self.target {
            let pages = pages::pages(env).len();
            if pages < MAX_PAGES {
                rows.push(Row::AddPage);
            }
            if pages > 1 {
                rows.push(Row::RemovePage);
            }
        }
        rows
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn run(cx: &mut Cx<'_, '_>, commands: Vec<Command>) {
        for command in commands {
            cx.command(command);
        }
    }

    fn choose(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(&row) = self.rows(&env).get(index) else {
            return;
        };
        match row {
            Row::Widget(at) => {
                let list = self.target.widgets(&env);
                let Some(id) = list.get(at) else {
                    return;
                };
                let mut options = Vec::new();
                if at > 0 {
                    options.push("up");
                }
                if at + 1 < list.len() {
                    options.push("down");
                }
                if has_settings(&env, id) {
                    options.push("settings");
                }
                options.push("remove");
                let title = env
                    .widgets
                    .title(id)
                    .map_or(id.as_str(), |t| env.i18n.tr(t));
                cx.command(choice::open(
                    CHOICE,
                    CHOSEN,
                    title,
                    &format!("{at}"),
                    OPTION_LABELS,
                    &options,
                ));
            }
            Row::Add => cx.command(Command::OpenScreen(ADD_SCREEN, self.target.params())),
            Row::AddPage => {
                let mut pages = pages::pages(&env);
                pages.push(alloc::vec![String::from(STATUS)]);
                self.target = Target::Page(pages.len() - 1);
                Self::run(cx, pages::store(&pages));
                self.list.focus = 0;
                self.redraw(cx);
            }
            Row::RemovePage => {
                let mut pages = pages::pages(&env);
                if let Target::Page(page) = self.target
                    && page < pages.len()
                    && pages.len() > 1
                {
                    pages.remove(page);
                    Self::run(cx, pages::store(&pages));
                }
                cx.command(Command::Back);
            }
        }
    }

    fn chosen(&mut self, option: &str, at: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let mut list = self.target.widgets(&env);
        if at >= list.len() {
            return;
        }
        match option {
            "up" if at > 0 => {
                list.swap(at, at - 1);
                self.list.focus = at - 1;
            }
            "down" if at + 1 < list.len() => {
                list.swap(at, at + 1);
                self.list.focus = at + 1;
            }
            "remove" => {
                list.remove(at);
            }
            "settings" => {
                let params = Params::new().with(params::ID, list[at].clone());
                cx.command(Command::OpenSlot(slots::SETTINGS, params));
                return;
            }
            _ => return,
        }
        let commands = self.target.store(&env, list);
        Self::run(cx, commands);
        self.redraw(cx);
    }
}

/// Whether widget `id` has settings of its own (a settings group with its
/// id, see `quireink_sdk::widgets`).
fn has_settings(env: &Env<'_>, id: &str) -> bool {
    env.settings.groups().any(|group| group.id.as_str() == id)
        && env
            .settings
            .specs()
            .any(|spec| env.settings.group_of(spec).as_str() == id)
}

impl Screen for EditScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let rows = self.rows(env);
        let title = self.target.title(env);
        self.header.draw(
            canvas,
            env,
            &style,
            &title,
            None,
            self.list.paging(rows.len()),
        );
        let list = self.target.widgets(env);
        let room = ui::icon_room(env, &style, true);
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| {
                let ink = if focused {
                    quireink_sdk::Color::WHITE
                } else {
                    quireink_sdk::Color::BLACK
                };
                let (icon, title, value) = match rows[index] {
                    Row::Widget(at) => {
                        let id = list.get(at).map_or("", String::as_str);
                        let name = env.widgets.title(id).map_or(id, |t| env.i18n.tr(t));
                        (None, name.to_string(), "›")
                    }
                    Row::Add => (Some("plus"), env.i18n.tr("home.edit.add").to_string(), "›"),
                    Row::AddPage => (
                        Some("plus"),
                        env.i18n.tr("home.edit.add-page").to_string(),
                        "",
                    ),
                    Row::RemovePage => (
                        Some("trash"),
                        env.i18n.tr("home.edit.remove-page").to_string(),
                        "",
                    ),
                };
                let rect = ui::draw_row_icon(canvas, env, &style, rect, room, icon, ink);
                ui::draw_value_row(canvas, env, &style, rect, focused, &title, value);
            },
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let len = self.rows(&env).len();
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
            self.choose(self.list.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.list.move_focus(delta, len) != ListChange::None {
            self.redraw(cx);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let env = *cx.env();
        let len = self.rows(&env).len();
        if let Some(action) =
            self.header
                .tap(&env, &self.style, gesture.start, self.list.paging(len))
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, len) {
            Some(index) => {
                self.list.focus = index;
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == CHOSEN {
            let option = params.get(choice::OPTION).unwrap_or_default().to_string();
            if let Some(at) = params.get(params::PATH).and_then(|p| p.parse().ok()) {
                self.chosen(&option, at, cx);
            }
        } else if *event == events::SETTING_CHANGED {
            self.redraw(cx);
        }
    }
}

/// Adding a widget: every widget not in the list yet (spaces always);
/// choosing one adds it at the end and goes back.
pub(crate) struct AddScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    target: Target,
    /// Ids and the translation keys of their names.
    widgets: Vec<(Id, String)>,
}

impl AddScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let target = target_of(params);
        let shown = target.widgets(env);
        let mut offered: Vec<(Id, String)> = env
            .widgets
            .iter()
            .filter(|(id, _)| {
                widgets::repeatable(id.as_str()) || !shown.iter().any(|s| s == id.as_str())
            })
            .map(|(id, title)| (id.clone(), title.to_string()))
            .collect();
        // The spaces after the widgets that show something.
        offered.sort_by_key(|(id, _)| widgets::repeatable(id.as_str()));
        Self {
            widgets: offered,
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
            target,
        }
    }

    fn add(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some((id, _)) = self.widgets.get(index) else {
            return;
        };
        let mut list = self.target.widgets(&env);
        list.push(id.as_str().to_string());
        for command in self.target.store(&env, list) {
            cx.command(command);
        }
        cx.command(Command::Back);
    }
}

impl Screen for AddScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.widgets.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("home.edit.add"),
            None,
            self.list.paging(len),
        );
        if len == 0 {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("home.edit.all-shown"),
            );
            return;
        }
        let widgets = &self.widgets;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let title = env.i18n.tr(&widgets[index].1);
                ui::draw_value_row(canvas, env, &style, rect, focused, title, "+");
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.widgets.len();
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
            self.add(self.list.focus, cx);
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
        let env = *cx.env();
        let len = self.widgets.len();
        if let Some(action) =
            self.header
                .tap(&env, &self.style, gesture.start, self.list.paging(len))
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, len) {
            Some(index) => {
                self.add(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}

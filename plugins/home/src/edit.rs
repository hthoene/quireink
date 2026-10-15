//! Editing a page of the home screen, as a list that buttons work as well
//! as taps: the page's widgets (choosing one offers to move it up or down
//! or to remove it), then *Add a widget*, *Add a page* and *Remove this
//! page*. Every change is kept at once.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, params};
use quireink_sdk::ui::choice;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen,
};

use crate::pages::{self, MAX_PAGES, STATUS};
use crate::{ADD_SCREEN, CHOICE};

/// A widget's options were chosen.
const CHOSEN: Id = Id::new_static("home.edit-chosen");
/// Parameter: the page edited (0 the first).
pub(crate) const PAGE: &str = "page";
const OPTION_LABELS: &str = "home.edit.";

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
    page: usize,
}

/// The page asked for: by number, or by its setting (from the settings).
pub(crate) fn page_of(params: &Params) -> usize {
    if let Some(page) = params.get(PAGE).and_then(|p| p.parse().ok()) {
        return page;
    }
    (0..MAX_PAGES)
        .find(|&i| params.get(params::ID) == Some(pages::setting(i).as_str()))
        .unwrap_or(0)
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
            page: page_of(params),
        }
    }

    fn rows(&self, env: &Env<'_>) -> Vec<Row> {
        let pages = pages::pages(env);
        let count = pages.get(self.page).map_or(0, Vec::len);
        let mut rows: Vec<Row> = (0..count).map(Row::Widget).collect();
        rows.push(Row::Add);
        if pages.len() < MAX_PAGES {
            rows.push(Row::AddPage);
        }
        if pages.len() > 1 {
            rows.push(Row::RemovePage);
        }
        rows
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn store(&self, pages: &[Vec<String>], cx: &mut Cx<'_, '_>) {
        for command in pages::store(pages) {
            cx.command(command);
        }
    }

    fn choose(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(&row) = self.rows(&env).get(index) else {
            return;
        };
        let mut pages = pages::pages(&env);
        match row {
            Row::Widget(at) => {
                let list = pages.get(self.page).cloned().unwrap_or_default();
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
            Row::Add => cx.command(Command::OpenScreen(
                ADD_SCREEN,
                Params::new().with(PAGE, format!("{}", self.page)),
            )),
            Row::AddPage => {
                pages.push(alloc::vec![String::from(STATUS)]);
                self.page = pages.len() - 1;
                self.store(&pages, cx);
                self.list.focus = 0;
                self.redraw(cx);
            }
            Row::RemovePage => {
                if self.page < pages.len() && pages.len() > 1 {
                    pages.remove(self.page);
                    self.store(&pages, cx);
                }
                cx.command(Command::Back);
            }
        }
    }

    fn chosen(&mut self, option: &str, at: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let mut pages = pages::pages(&env);
        let Some(list) = pages.get_mut(self.page) else {
            return;
        };
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
            _ => return,
        }
        self.store(&pages, cx);
        self.redraw(cx);
    }
}

impl Screen for EditScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let rows = self.rows(env);
        let title = env.i18n.format(
            "home.edit.title",
            &[("page", Arg::Int(self.page as i64 + 1))],
        );
        self.header.draw(
            canvas,
            env,
            &style,
            &title,
            None,
            self.list.paging(rows.len()),
        );
        let list = pages::pages(env)
            .get(self.page)
            .cloned()
            .unwrap_or_default();
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

/// Adding a widget to a page: every widget not on it yet; choosing one
/// adds it at the end and goes back.
pub(crate) struct AddScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    page: usize,
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
        let page = page_of(params);
        let shown = pages::pages(env).get(page).cloned().unwrap_or_default();
        Self {
            widgets: env
                .widgets
                .iter()
                .filter(|(id, _)| !shown.iter().any(|s| s == id.as_str()))
                .map(|(id, title)| (id.clone(), title.to_string()))
                .collect(),
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
            page,
        }
    }

    fn add(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some((id, _)) = self.widgets.get(index) else {
            return;
        };
        let pages = pages::pages(&env);
        let pages = pages::with_widget(pages, self.page, id.as_str(), "");
        for command in pages::store(&pages) {
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

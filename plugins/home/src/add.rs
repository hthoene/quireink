//! Adding to a page of the home screen: every widget and tool not on it
//! yet, then *Add a page* and *Remove this page*. A widget or tool goes
//! into the first free cells of the page (a widget as wide as the page
//! and as high as it needs, a tool one cell); the page is kept at once.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::actions;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::widgets;
use quireink_sdk::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode, Screen,
};

use crate::grid::{self, Item, What};
use crate::pages::{self, MAX_PAGES, STATUS, STATUS_BAR, TOOLS};

/// Parameter: the page added to (0 the first).
pub(crate) const PAGE: &str = "page";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    /// A widget or tool, and the translation key of its name.
    Add(What, String),
    AddPage,
    RemovePage,
}

pub(crate) struct AddScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    page: usize,
    rows: Vec<Row>,
}

impl AddScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let page = params.get(PAGE).and_then(|p| p.parse().ok()).unwrap_or(0);
        let all = pages::pages(env);
        let shown = all
            .get(page)
            .map(|items| pages::expand(env, items))
            .unwrap_or_default();
        let on_page = |what: &What| shown.iter().any(|i| i.what == *what);
        let mut rows: Vec<Row> = env
            .widgets
            .iter()
            .filter(|(id, _)| {
                let id = id.as_str();
                // Empty cells are the grid's spaces; the status line is
                // above it where shown.
                !widgets::repeatable(id)
                    && id != TOOLS
                    && !(id == STATUS && env.settings.bool(STATUS_BAR.as_str()))
            })
            .map(|(id, title)| (What::Widget(id.as_str().to_string()), title.to_string()))
            .chain(
                grid::tools(env)
                    .into_iter()
                    .map(|(action, title)| (What::Tool(action), title)),
            )
            .filter(|(what, _)| !on_page(what))
            .map(|(what, title)| Row::Add(what, title))
            .collect();
        if all.len() < MAX_PAGES {
            rows.push(Row::AddPage);
        }
        if all.len() > 1 {
            rows.push(Row::RemovePage);
        }
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
            page,
            rows,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let mut all = pages::pages(&env);
        match self.rows.get(index) {
            Some(Row::Add(what, _)) => {
                let Some(items) = all.get_mut(self.page) else {
                    return;
                };
                let (_, geometry, _) = crate::frame(&env);
                let placed = grid::place(&env, &geometry, &pages::expand(&env, items));
                let taken: Vec<grid::Cells> = placed.iter().map(|(_, c)| *c).collect();
                let (w, h) = geometry.size_of(&env, what);
                let Some(at) = (1..=w)
                    .rev()
                    .find_map(|w| grid::free_place(&geometry, &taken, w, h))
                else {
                    let message = env.i18n.tr("home.grid.no-room").to_string();
                    cx.command(Command::Notify(message));
                    return;
                };
                // The page as shown, every item with its place, and the new one.
                *items = placed
                    .into_iter()
                    .map(|(item, cells)| Item {
                        what: item.what,
                        at: Some(cells),
                    })
                    .collect();
                items.push(Item {
                    what: what.clone(),
                    at: Some(at),
                });
            }
            Some(Row::AddPage) => all.push(Vec::new()),
            Some(Row::RemovePage) => {
                if self.page < all.len() && all.len() > 1 {
                    all.remove(self.page);
                }
            }
            None => return,
        }
        for command in pages::store(&all) {
            cx.command(command);
        }
        cx.command(Command::Back);
    }
}

impl Screen for AddScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.rows.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("home.edit.add"),
            None,
            self.list.paging(len),
        );
        let rows = &self.rows;
        let room = ui::icon_room(env, &style, true);
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let ink = if focused {
                    quireink_sdk::Color::WHITE
                } else {
                    quireink_sdk::Color::BLACK
                };
                let (icon, title, value) = match &rows[index] {
                    Row::Add(What::Widget(_), title) => (None, env.i18n.tr(title).to_string(), "+"),
                    Row::Add(What::Tool(action), title) => (
                        env.menus.icon(action.as_str()),
                        env.i18n.tr(title).to_string(),
                        "+",
                    ),
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
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.rows.len();
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
        let len = self.rows.len();
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
}

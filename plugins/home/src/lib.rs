//! The home screen: pages of widgets and tools in a grid, like a phone's.
//!
//! - **Pages**: each a grid of cells (4 columns by default, rows to fit
//!   the screen) in a setting of its own (see `grid`, `pages`): by default
//!   the clock, the book being read and the recent ones, then a page with
//!   all tools. The status line sits above the grid. Swipes and the page
//!   buttons go from page to page; dots below show where one is.
//! - **Items**: widgets (the home screen's own, see `blocks`, and any
//!   plugin's, `Registrar::add_widget`) and tools (actions, as icons),
//!   each taking a rectangle of cells; empty cells stay empty.
//! - **Targets**: what an item does when tapped (open a book, a tool).
//!   With buttons, left and right (or up and down) go from target to
//!   target and confirm runs it; the first press shows where the focus is.
//! - **Editing**: the pencil below the page (reached with the buttons too)
//!   or a long press shows the grid: drag an item to move it, by its
//!   corner to size it; tap it for its options (move or size it with the
//!   buttons, its settings, remove it). *Add* lists what can be added,
//!   and adds or removes pages (see `add`).
//! - **Tools**: the entries of the `home.actions` menu (the library by
//!   default) and of the tools menu; plugins add their own with
//!   `Registrar::add_menu_entry`.
//!
//! The recently read books and their covers come from the library service
//! (`quireink_sdk::library`), which follows the standard reading events,
//! so any reader plugin works.
//! The home screen is bound to `core.home`; replace it with any other
//! screen by configuration or with a plugin of higher priority.

#![no_std]

extern crate alloc;

mod add;
mod blocks;
mod covers;
mod edit;
mod grid;
mod pages;
mod recent;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, params, slots};
use quireink_sdk::ui::choice::{self, Choice};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, MenuEntry,
    Params, Plugin, Point, PointerEvent, PointerPhase, Priority, Rect, RefreshMode, RegisterError,
    Registrar, SDK_VERSION, Screen, ScreenSpec, Version,
};

use grid::{Cells, Geometry, Item, What};

pub const PLUGIN: Id = Id::new_static("home");
pub const SCREEN: Id = Id::new_static("home.dashboard");
/// Shortcuts at the head of the home screen's grid of tools.
pub const ACTIONS: Id = Id::new_static("home.actions");
/// Editing a list of widgets (e.g. the sleep screen's).
pub(crate) const EDIT_SCREEN: Id = Id::new_static("home.edit");
/// Adding a widget or tool to a page.
pub(crate) const ADD_SCREEN: Id = Id::new_static("home.add-widget");
/// Adding a widget to a list of widgets.
pub(crate) const LIST_ADD_SCREEN: Id = Id::new_static("home.add-to-list");
pub(crate) const CHOICE: Id = Id::new_static("home.choice");
/// An item's options were chosen.
const CHOSEN: Id = Id::new_static("home.item-chosen");

/// The home screen plugin.
#[derive(Debug, Default)]
pub struct Home;

impl Plugin for Home {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Home screen"),
            description: Cow::Borrowed(
                "The start page: pages of widgets, such as the book being read and the tools.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("home.title"),
            factory: Box::new(|env, params| Box::new(Dashboard::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: EDIT_SCREEN,
            title: Cow::Borrowed("home.edit.screen"),
            factory: Box::new(|env, params| Box::new(edit::EditScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: ADD_SCREEN,
            title: Cow::Borrowed("home.edit.add"),
            factory: Box::new(|env, params| Box::new(add::AddScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: LIST_ADD_SCREEN,
            title: Cow::Borrowed("home.edit.add"),
            factory: Box::new(|env, params| Box::new(edit::AddScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("home.edit.screen"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.bind_slot(slots::HOME, SCREEN, Priority::DEFAULT)?;
        r.add_menu_entry(MenuEntry {
            menu: ACTIONS,
            item: actions::LIBRARY,
            order: 10,
        })?;
        pages::register(r)?;
        blocks::register(r)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// An item on the page: where it is and what it showed when drawn.
#[derive(Debug)]
struct Shown {
    item: Item,
    cells: Cells,
    area: Rect,
    shown: Option<String>,
}

/// What editing is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Choosing an item (tap, or the buttons' focus).
    Choose,
    /// The buttons move the item with the focus a cell at a time.
    Move,
    /// The buttons make it wider, narrower, taller or lower.
    Resize,
}

/// An item dragged with a finger: from where, and where it would go.
#[derive(Debug, Clone, Copy)]
struct Drag {
    index: usize,
    /// Its size is changed (dragged by the corner), not its place.
    resize: bool,
    start: Point,
    to: Cells,
    moved: bool,
}

/// Editing the page in place: the grid shows, items are moved and sized
/// by dragging, or with the buttons through their options.
#[derive(Debug)]
struct Edit {
    mode: Mode,
    drag: Option<Drag>,
}

#[derive(Debug)]
struct Dashboard {
    style: UiStyle,
    page: usize,
    pages: usize,
    geometry: Geometry,
    /// The status line above the grid, if shown.
    status: Option<Rect>,
    items: Vec<Shown>,
    /// Items of the page without room on it, kept as they are.
    unplaced: Vec<Item>,
    /// Where taps and the buttons' focus go, in order; the footer's last.
    targets: Vec<(Rect, Target)>,
    /// The target with the focus, shown once buttons are used.
    focus: usize,
    focus_shown: bool,
    /// The dots and the pencil (when editing: add and done) below.
    footer: Rect,
    pencil: Rect,
    plus: Rect,
    edit: Option<Edit>,
}

/// What a target does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Run(Command),
    /// An item of the page, when editing: its index.
    Item(usize),
    /// Editing begins (the pencil) or ends (done).
    Edit,
    Add,
}

/// Where the parts of the home screen go: the status line, the grid, the
/// footer.
fn frame(env: &Env<'_>) -> (Option<Rect>, Geometry, Rect) {
    let style = UiStyle::new(env);
    let display = env.device.display;
    let margin = style.margin as u16;
    let gap = style.gap as u16;
    let footer_height = style.row as u16;
    let footer = Rect::new(
        0,
        display.height - footer_height,
        display.width,
        footer_height,
    );
    let mut top = gap / 2;
    let status = env.settings.bool(pages::STATUS_BAR.as_str()).then(|| {
        let height = blocks::status_height(env, display.width);
        let r = Rect::new(
            margin,
            top,
            display.width.saturating_sub(2 * margin),
            height,
        );
        top += height;
        r
    });
    let area = Rect::new(
        margin - gap / 2,
        top,
        display.width.saturating_sub(2 * margin) + gap,
        footer.y.saturating_sub(top),
    );
    (status, Geometry::new(env, area), footer)
}

impl Dashboard {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        // Opened to edit a page (from its setting).
        let editing = params.get(params::ID).and_then(pages::page_of);
        let (status, geometry, footer) = frame(env);
        let mut dashboard = Self {
            style: UiStyle::new(env),
            page: editing.unwrap_or(0),
            pages: 1,
            geometry,
            status,
            items: Vec::new(),
            unplaced: Vec::new(),
            targets: Vec::new(),
            focus: 0,
            focus_shown: !env.device.input.touch,
            footer,
            pencil: Rect::default(),
            plus: Rect::default(),
            edit: editing.map(|_| Edit {
                mode: Mode::Choose,
                drag: None,
            }),
        };
        dashboard.layout(env);
        dashboard
    }

    /// Places the page's items in the grid, and the targets.
    fn layout(&mut self, env: &Env<'_>) {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let (status, geometry, footer) = frame(env);
        let pages = pages::pages(env);
        self.pages = pages.len();
        self.page = self.page.min(self.pages - 1);
        let items = pages::expand(env, &pages[self.page]);
        let placed = grid::place(env, &geometry, &items);
        self.unplaced = items
            .iter()
            .filter(|i| !placed.iter().any(|(p, _)| p == *i))
            .cloned()
            .collect();
        self.items = placed
            .into_iter()
            .map(|(item, cells)| Shown {
                shown: match &item.what {
                    What::Widget(id) => env.widgets.signature(id, env),
                    What::Tool(_) => None,
                },
                area: geometry.rect(cells),
                item,
                cells,
            })
            .collect();
        self.targets.clear();
        let editing = self.edit.is_some();
        for (index, shown) in self.items.iter().enumerate() {
            if editing {
                self.targets.push((shown.area, Target::Item(index)));
                continue;
            }
            match &shown.item.what {
                What::Widget(id) => {
                    for target in env.widgets.targets(id, env, shown.area) {
                        self.targets
                            .push((target.area, Target::Run(target.command)));
                    }
                }
                What::Tool(action) => self
                    .targets
                    .push((shown.area, Target::Run(Command::RunAction(action.clone())))),
            }
        }
        // The pencil (done, when editing) at the right of the footer; add
        // at the left when editing.
        let margin = style.margin as u16;
        let side = footer.height;
        self.pencil = Rect::new(display.width - margin - side, footer.y, side, side);
        self.plus = Rect::new(margin, footer.y, side, side);
        if editing {
            self.targets.push((self.plus, Target::Add));
        }
        self.targets.push((self.pencil, Target::Edit));
        self.status = status;
        self.geometry = geometry;
        self.footer = footer;
        self.focus = self.focus.min(self.targets.len() - 1);
        self.style = style;
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>, mode: RefreshMode) {
        cx.request_redraw();
        cx.request_refresh(mode);
    }

    fn turn(&mut self, forward: bool, cx: &mut Cx<'_, '_>) {
        let to = if forward {
            (self.page + 1 < self.pages).then_some(self.page + 1)
        } else {
            self.page.checked_sub(1)
        };
        if let Some(to) = to {
            self.page = to;
            self.focus = 0;
            let env = *cx.env();
            self.layout(&env);
            self.redraw(cx, RefreshMode::Partial);
        }
    }

    /// Moves the focus to `to`, redrawing both targets.
    fn focus_on(&mut self, to: usize, cx: &mut Cx<'_, '_>) {
        if let Some((area, _)) = self.targets.get(self.focus) {
            cx.request_redraw_area(grown(*area, self.style.line * 4));
        }
        self.focus = to;
        if let Some((area, _)) = self.targets.get(to) {
            cx.request_redraw_area(grown(*area, self.style.line * 4));
        }
        cx.request_refresh(RefreshMode::Fast);
    }

    /// The target nearest below (or above) the focused one.
    fn vertical(&self, down: bool) -> Option<usize> {
        let (from, _) = self.targets.get(self.focus)?;
        let cx = i32::from(from.x) + i32::from(from.width) / 2;
        self.targets
            .iter()
            .enumerate()
            .filter(|(_, (r, _))| {
                if down {
                    r.y >= from.y + from.height
                } else {
                    r.y + r.height <= from.y
                }
            })
            .min_by_key(|(_, (r, _))| {
                let dy = (i32::from(r.y) - i32::from(from.y)).abs();
                let dx = (i32::from(r.x) + i32::from(r.width) / 2 - cx).abs();
                dy * 4 + dx
            })
            .map(|(i, _)| i)
    }

    fn run(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some((_, target)) = self.targets.get(index).cloned() else {
            return;
        };
        match target {
            Target::Run(command) => cx.command(command),
            Target::Item(item) => self.options(item, cx),
            Target::Edit => self.set_editing(self.edit.is_none(), cx),
            Target::Add => cx.command(Command::OpenScreen(
                ADD_SCREEN,
                Params::new().with(add::PAGE, format!("{}", self.page)),
            )),
        }
    }

    /// Begins or ends editing the page.
    fn set_editing(&mut self, on: bool, cx: &mut Cx<'_, '_>) {
        self.edit = on.then_some(Edit {
            mode: Mode::Choose,
            drag: None,
        });
        self.focus = 0;
        let env = *cx.env();
        self.layout(&env);
        self.redraw(cx, RefreshMode::Partial);
    }

    /// Offers what can be done with item `index`.
    fn options(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(shown) = self.items.get(index) else {
            return;
        };
        let mut options = alloc::vec!["move", "resize"];
        if let What::Widget(id) = &shown.item.what
            && edit::has_settings(&env, id)
        {
            options.push("settings");
        }
        options.push("remove");
        let title = item_title(&env, &shown.item.what);
        cx.command(choice::open(
            CHOICE,
            CHOSEN,
            &title,
            &format!("{index}"),
            "home.grid.",
            &options,
        ));
    }

    /// Keeps the page as its items are now, every item with its place.
    fn store(&self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let mut pages = pages::pages(&env);
        let mut items: Vec<Item> = self
            .items
            .iter()
            .map(|s| Item {
                what: s.item.what.clone(),
                at: Some(s.cells),
            })
            .collect();
        items.extend(self.unplaced.iter().cloned());
        if let Some(page) = pages.get_mut(self.page) {
            *page = items;
        }
        for command in pages::store(&pages) {
            cx.command(command);
        }
    }

    /// Whether item `index` may take the cells `to`.
    fn free(&self, index: usize, to: Cells) -> bool {
        to.fits(self.geometry.columns, self.geometry.rows)
            && !self
                .items
                .iter()
                .enumerate()
                .any(|(i, s)| i != index && s.cells.overlaps(&to))
    }

    /// Puts item `index` at `to` if it may go there; `true` if it did.
    fn set_cells(&mut self, index: usize, to: Cells, cx: &mut Cx<'_, '_>) -> bool {
        if !self.free(index, to) {
            return false;
        }
        let Some(shown) = self.items.get_mut(index) else {
            return false;
        };
        cx.request_redraw_area(grown(shown.area, self.style.line * 4));
        shown.cells = to;
        shown.area = self.geometry.rect(to);
        cx.request_redraw_area(grown(shown.area, self.style.line * 4));
        let area = shown.area;
        if let Some(target) = self.targets.get_mut(index) {
            target.0 = area;
        }
        true
    }

    /// The edit target at a point: the item there, and whether at its
    /// corner (to size it).
    fn item_at(&self, point: Point) -> Option<(usize, bool)> {
        let index = self.items.iter().position(|s| s.area.contains(point))?;
        let corner = handle(self.items[index].area, &self.style).contains(point);
        Some((index, corner))
    }

    fn target_at(&self, point: Point) -> Option<usize> {
        self.targets.iter().position(|(r, _)| r.contains(point))
    }

    /// The cells an item dragged to `point` would take.
    fn dragged_to(&self, drag: &Drag, point: Point) -> Option<Cells> {
        let from = self.items.get(drag.index)?.cells;
        let (col, row) = self.geometry.cell_at(point.x, point.y)?;
        if drag.resize {
            return Some(Cells {
                w: (col + 1).saturating_sub(from.col).max(1),
                h: (row + 1).saturating_sub(from.row).max(1),
                ..from
            });
        }
        let (start_col, start_row) = self.geometry.cell_at(drag.start.x, drag.start.y)?;
        let col = (i32::from(from.col) + i32::from(col) - i32::from(start_col)).max(0) as u8;
        let row = (i32::from(from.row) + i32::from(row) - i32::from(start_row)).max(0) as u8;
        Some(Cells { col, row, ..from })
    }

    fn draw_footer(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let f = self.footer;
        // A dot a page, the current one filled.
        if self.pages > 1 {
            let dot = (style.gap * 2 / 3).max(6);
            let spacing = dot * 2;
            let total = dot * self.pages as u32 + (spacing - dot) * (self.pages as u32 - 1);
            let mut x = i32::from(f.x) + (i32::from(f.width) - total as i32) / 2;
            let y = i32::from(f.y) + (i32::from(f.height) - dot as i32) / 2;
            for page in 0..self.pages {
                if page == self.page {
                    canvas.fill_rect(x, y, dot, dot, Color::BLACK);
                } else {
                    canvas.stroke_rect(x, y, dot, dot, style.line.max(1), Color::BLACK);
                }
                x += spacing as i32;
            }
        }
        let size = style.title;
        let mut icon = |name: &str, area: Rect, ink: Color| {
            let w = ui::icon_width(env, name, size);
            let x = f32::from(area.x) + (f32::from(area.width) - w) / 2.0;
            ui::draw_icon(canvas, env, name, size, x, area, ink);
        };
        if self.edit.is_some() {
            icon("plus", self.plus, Color::BLACK);
            icon("check", self.pencil, Color::BLACK);
        } else {
            icon("pencil", self.pencil, style.faint(60));
        }
    }

    /// The grid's cells, lightly, while editing.
    fn draw_cells(&self, canvas: &mut Canvas<'_>) {
        let g = self.geometry;
        let gray = Color::gray(200);
        for row in 0..g.rows {
            for col in 0..g.columns {
                let r = g.rect(Cells {
                    col,
                    row,
                    w: 1,
                    h: 1,
                });
                canvas.stroke_rect(
                    i32::from(r.x),
                    i32::from(r.y),
                    u32::from(r.width),
                    u32::from(r.height),
                    1,
                    gray,
                );
            }
        }
    }

    /// Ends moving or sizing with the buttons, keeping the page.
    fn end_mode(&mut self, cx: &mut Cx<'_, '_>) {
        if let Some(edit) = &mut self.edit {
            edit.mode = Mode::Choose;
        }
        self.store(cx);
        self.redraw(cx, RefreshMode::Partial);
    }
}

/// The corner of an item that sizes it when dragged.
fn handle(area: Rect, style: &UiStyle) -> Rect {
    let side = (style.row / 2).max(16) as u16;
    Rect::new(
        (area.x + area.width).saturating_sub(side),
        (area.y + area.height).saturating_sub(side),
        side,
        side,
    )
}

/// An item's name: the widget's, or the tool's.
fn item_title(env: &Env<'_>, what: &What) -> String {
    match what {
        What::Widget(id) => env
            .widgets
            .title(id)
            .map_or(id.as_str(), |t| env.i18n.tr(t))
            .to_string(),
        What::Tool(action) => grid::tools(env)
            .into_iter()
            .find(|(id, _)| id == action)
            .map_or(action.as_str().to_string(), |(_, title)| {
                env.i18n.tr(&title).to_string()
            }),
    }
}

/// `r` grown by `by` on every side (for the focus frame).
fn grown(r: Rect, by: u32) -> Rect {
    let by = by as u16;
    Rect::new(
        r.x.saturating_sub(by),
        r.y.saturating_sub(by),
        r.width + 2 * by,
        r.height + 2 * by,
    )
}

fn frame_rect(canvas: &mut Canvas<'_>, r: Rect, width: u32) {
    canvas.stroke_rect(
        i32::from(r.x),
        i32::from(r.y),
        u32::from(r.width),
        u32::from(r.height),
        width,
        Color::BLACK,
    );
}

impl Screen for Dashboard {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        if let Some(status) = self.status {
            blocks::draw_status(canvas, env, status);
        }
        let editing = self.edit.is_some();
        if editing {
            self.draw_cells(canvas);
        }
        for shown in &self.items {
            ui::fill(canvas, shown.area, Color::WHITE);
            match &shown.item.what {
                What::Widget(id) => env.widgets.draw(id, canvas, env, shown.area),
                What::Tool(action) => blocks::draw_tool(canvas, env, shown.area, action),
            }
            if editing {
                // Its frame, and the corner that sizes it.
                frame_rect(canvas, shown.area, self.style.line.max(1));
                let h = handle(shown.area, &self.style);
                canvas.fill_rect(
                    i32::from(h.x),
                    i32::from(h.y),
                    u32::from(h.width),
                    u32::from(h.height),
                    Color::BLACK,
                );
            }
        }
        self.draw_footer(canvas, env);
        // Where a dragged item would go.
        if let Some(Edit {
            drag: Some(drag), ..
        }) = &self.edit
            && drag.moved
        {
            let width = if self.free(drag.index, drag.to) {
                self.style.line * 4
            } else {
                self.style.line
            };
            frame_rect(canvas, self.geometry.rect(drag.to), width);
        }
        // The focus: a frame around its target, once buttons are used.
        if self.focus_shown
            && let Some((area, _)) = self.targets.get(self.focus)
        {
            let frame = grown(*area, self.style.line * 2);
            frame_rect(canvas, frame, self.style.line * 3);
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        // The reader may have changed the recent books meanwhile.
        let env = *cx.env();
        self.layout(&env);
        cx.request_redraw();
        cx.wake_at(env.now);
    }

    /// Redraws widgets whose content changed (the time each minute) and
    /// looks up the covers not known yet, one a time.
    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let mut changed = false;
        if let Some(status) = self.status {
            cx.request_redraw_area(status);
        }
        for shown in &mut self.items {
            let What::Widget(id) = &shown.item.what else {
                continue;
            };
            let now = env.widgets.signature(id, &env);
            if now.is_some() && now != shown.shown {
                shown.shown = now;
                cx.request_redraw_area(shown.area);
                changed = true;
            }
        }
        let missing = blocks::wanted_covers(&env)
            .into_iter()
            .find(|path| covers::get(env.storage, path).is_none());
        if let Some(path) = missing {
            if covers::look_up(&env, &path).is_some() {
                for shown in &self.items {
                    if shown.item.what == What::Widget(pages::CONTINUE.into())
                        || shown.item.what == What::Widget(pages::RECENT.into())
                    {
                        cx.request_redraw_area(shown.area);
                        changed = true;
                    }
                }
            }
            cx.wake_at(env.now.add_millis(20));
        } else {
            let next = quireink_sdk::time::next_minute(&env).unwrap_or(env.now.add_millis(60_000));
            cx.wake_at(next);
        }
        if changed {
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == CHOSEN {
            let option = params.get(choice::OPTION).unwrap_or_default();
            let Some(index) = params
                .get(params::PATH)
                .and_then(|p| p.parse::<usize>().ok())
            else {
                return;
            };
            if index >= self.items.len() {
                return;
            }
            let env = *cx.env();
            match option {
                "move" | "resize" => {
                    if let Some(edit) = &mut self.edit {
                        edit.mode = if option == "move" {
                            Mode::Move
                        } else {
                            Mode::Resize
                        };
                    }
                    self.focus = index;
                    self.focus_shown = true;
                    let message = env.i18n.tr(&format!("home.grid.{option}.help")).to_string();
                    cx.command(Command::Notify(message));
                    self.redraw(cx, RefreshMode::Fast);
                }
                "settings" => {
                    if let What::Widget(id) = &self.items[index].item.what {
                        let params = Params::new().with(params::ID, id.clone());
                        cx.command(Command::OpenSlot(slots::SETTINGS, params));
                    }
                }
                "remove" => {
                    self.items.remove(index);
                    self.store(cx);
                }
                _ => {}
            }
            return;
        }
        if *event == events::SETTING_CHANGED || *event == events::STATUS_CHANGED {
            let env = *cx.env();
            self.layout(&env);
            cx.request_redraw();
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        // Moving or sizing an item with the buttons.
        if let Some(Edit { mode, .. }) = self.edit
            && mode != Mode::Choose
            && let Some(Target::Item(index)) = self.targets.get(self.focus).map(|t| t.1.clone())
        {
            if *a == actions::CONFIRM || *a == actions::BACK {
                self.end_mode(cx);
                return Handled::Yes;
            }
            let Some(c) = self.items.get(index).map(|s| s.cells) else {
                return Handled::Yes;
            };
            let step = |n: u8, more: bool| {
                if more {
                    n.saturating_add(1)
                } else {
                    n.saturating_sub(1)
                }
            };
            let to = match (mode, a) {
                (Mode::Move, a) if *a == actions::LEFT || *a == actions::PREVIOUS => Cells {
                    col: step(c.col, false),
                    ..c
                },
                (Mode::Move, a) if *a == actions::RIGHT || *a == actions::NEXT => Cells {
                    col: step(c.col, true),
                    ..c
                },
                (Mode::Move, a) if *a == actions::UP => Cells {
                    row: step(c.row, false),
                    ..c
                },
                (Mode::Move, a) if *a == actions::DOWN => Cells {
                    row: step(c.row, true),
                    ..c
                },
                (Mode::Resize, a) if *a == actions::LEFT || *a == actions::PREVIOUS => Cells {
                    w: step(c.w, false).max(1),
                    ..c
                },
                (Mode::Resize, a) if *a == actions::RIGHT || *a == actions::NEXT => Cells {
                    w: step(c.w, true),
                    ..c
                },
                (Mode::Resize, a) if *a == actions::UP => Cells {
                    h: step(c.h, false).max(1),
                    ..c
                },
                (Mode::Resize, a) if *a == actions::DOWN => Cells {
                    h: step(c.h, true),
                    ..c
                },
                _ => return Handled::No,
            };
            if to != c && self.set_cells(index, to, cx) {
                cx.request_refresh(RefreshMode::Fast);
            }
            return Handled::Yes;
        }
        if *a == actions::BACK && self.edit.is_some() {
            self.set_editing(false, cx);
            return Handled::Yes;
        }
        if *a == actions::NEXT_PAGE || *a == actions::PREV_PAGE {
            self.turn(*a == actions::NEXT_PAGE, cx);
            return Handled::Yes;
        }
        let moves = [
            actions::UP,
            actions::DOWN,
            actions::LEFT,
            actions::RIGHT,
            actions::NEXT,
            actions::PREVIOUS,
            actions::CONFIRM,
        ];
        if !moves.contains(a) {
            return Handled::No;
        }
        if !self.focus_shown {
            // The first press shows where the focus is.
            self.focus_shown = true;
            self.focus_on(self.focus, cx);
            return Handled::Yes;
        }
        let last = self.targets.len() - 1;
        let to = if *a == actions::CONFIRM {
            self.run(self.focus, cx);
            return Handled::Yes;
        } else if *a == actions::RIGHT || *a == actions::NEXT {
            if self.focus == last {
                0
            } else {
                self.focus + 1
            }
        } else if *a == actions::LEFT || *a == actions::PREVIOUS {
            if self.focus == 0 {
                last
            } else {
                self.focus - 1
            }
        } else {
            match self.vertical(*a == actions::DOWN) {
                Some(to) => to,
                None => return Handled::Yes,
            }
        };
        self.focus_on(to, cx);
        Handled::Yes
    }

    /// While editing, a finger on an item drags it (by its corner: sizes
    /// it); lifted where it was, it offers the item's options.
    fn on_pointer(&mut self, event: &PointerEvent, cx: &mut Cx<'_, '_>) -> Handled {
        if self.edit.is_none() {
            return Handled::No;
        }
        match event.phase {
            PointerPhase::Down => {
                let Some((index, corner)) = self.item_at(event.point) else {
                    return Handled::No;
                };
                let Some(from) = self.items.get(index).map(|s| s.cells) else {
                    return Handled::No;
                };
                // Its corner sizes it.
                let resize = corner;
                if let Some(edit) = &mut self.edit {
                    edit.drag = Some(Drag {
                        index,
                        resize,
                        start: event.point,
                        to: from,
                        moved: false,
                    });
                }
                Handled::Yes
            }
            PointerPhase::Move => {
                let Some(mut drag) = self.edit.as_ref().and_then(|e| e.drag) else {
                    return Handled::Yes;
                };
                let Some(to) = self.dragged_to(&drag, event.point) else {
                    return Handled::Yes;
                };
                if to != drag.to || !drag.moved {
                    let before = self.geometry.rect(drag.to);
                    cx.request_redraw_area(grown(before, self.style.line * 4));
                    drag.to = to;
                    drag.moved |= to != self.items[drag.index].cells;
                    cx.request_redraw_area(grown(self.geometry.rect(to), self.style.line * 4));
                    cx.request_refresh(RefreshMode::Fast);
                    if let Some(edit) = &mut self.edit {
                        edit.drag = Some(drag);
                    }
                }
                Handled::Yes
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                let Some(drag) = self.edit.as_mut().and_then(|e| e.drag.take()) else {
                    return Handled::Yes;
                };
                if !drag.moved {
                    // A tap: the item gets the focus and offers its options.
                    let target = self
                        .targets
                        .iter()
                        .position(|t| t.1 == Target::Item(drag.index));
                    if let Some(target) = target {
                        self.focus = target;
                        self.focus_shown = true;
                    }
                    if event.phase == PointerPhase::Up {
                        self.options(drag.index, cx);
                    }
                    self.redraw(cx, RefreshMode::Fast);
                    return Handled::Yes;
                }
                if event.phase == PointerPhase::Up && self.set_cells(drag.index, drag.to, cx) {
                    self.store(cx);
                }
                self.redraw(cx, RefreshMode::Partial);
                Handled::Yes
            }
        }
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        match gesture.kind {
            GestureKind::Tap => match self.target_at(gesture.start) {
                Some(index) => {
                    self.focus = index;
                    self.run(index, cx);
                    Handled::Yes
                }
                None => Handled::No,
            },
            GestureKind::LongPress if self.edit.is_none() => {
                self.set_editing(true, cx);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

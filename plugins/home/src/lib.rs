//! The home screen: pages of widgets.
//!
//! - **Pages**: each a list of widgets in a setting of its own (see
//!   `pages`): by default the status line, the date, the book being read
//!   and the recent ones, then a second page with all tools. Swipes and
//!   the page buttons go from page to page; dots below show where one is.
//! - **Widgets**: the home screen's own (see `blocks`) and any plugin's
//!   (`Registrar::add_widget`), e.g. today's reading.
//! - **Targets**: what a widget does when tapped (open a book, a tool).
//!   With buttons, left and right (or up and down) go from target to
//!   target and confirm runs it; the first press shows where the focus is.
//! - **Editing**: the pencil below the page (reached with the buttons too)
//!   or a long press opens a list of the page's widgets to move, remove
//!   and add, and to add or remove pages (see `edit`).
//! - **Shortcuts**: the entries of the `home.actions` menu (the library by
//!   default) lead the grid of tools; plugins add their own with
//!   `Registrar::add_menu_entry`.
//!
//! The recently read books come from the standard reading events
//! (`core.book-opened`, `core.page-turned`), so any reader plugin works.
//! The home screen is bound to `core.home`; replace it with any other
//! screen by configuration or with a plugin of higher priority.

#![no_std]

extern crate alloc;

mod blocks;
mod covers;
mod edit;
mod pages;
mod recent;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, slots};
use quireink_sdk::ui::choice::Choice;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::widgets;
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, MenuEntry,
    Params, Plugin, Point, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION,
    Screen, ScreenSpec, Version,
};

pub const PLUGIN: Id = Id::new_static("home");
pub const SCREEN: Id = Id::new_static("home.dashboard");
/// Shortcuts at the head of the home screen's grid of tools.
pub const ACTIONS: Id = Id::new_static("home.actions");
/// Editing a page.
pub(crate) const EDIT_SCREEN: Id = Id::new_static("home.edit");
/// Adding a widget to a page.
pub(crate) const ADD_SCREEN: Id = Id::new_static("home.add-widget");
pub(crate) const CHOICE: Id = Id::new_static("home.choice");

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
            factory: Box::new(|env, _| Box::new(Dashboard::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: EDIT_SCREEN,
            title: Cow::Borrowed("home.edit.screen"),
            factory: Box::new(|env, params| Box::new(edit::EditScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: ADD_SCREEN,
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
        for event in [events::BOOK_OPENED, events::PAGE_TURNED] {
            r.subscribe(
                event,
                Box::new(|env, params, _| recent::record(env, params)),
            )?;
        }
        r.subscribe(
            events::BOOK_MOVED,
            Box::new(|env, params, _| recent::moved(env, params)),
        )?;
        pages::register(r)?;
        blocks::register(r)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// A widget on the page: where it is and what it showed when drawn.
#[derive(Debug)]
struct Placed {
    id: String,
    area: Rect,
    shown: Option<String>,
}

#[derive(Debug)]
struct Dashboard {
    style: UiStyle,
    page: usize,
    pages: usize,
    widgets: Vec<Placed>,
    /// Where taps and the buttons' focus go, in order; the pencil last.
    targets: Vec<(Rect, Command)>,
    /// The target with the focus, shown once buttons are used.
    focus: usize,
    focus_shown: bool,
    /// The dots and the pencil below the page.
    footer: Rect,
    pencil: Rect,
}

impl Dashboard {
    fn new(env: &Env<'_>) -> Self {
        let mut dashboard = Self {
            style: UiStyle::new(env),
            page: 0,
            pages: 1,
            widgets: Vec::new(),
            targets: Vec::new(),
            focus: 0,
            focus_shown: !env.device.input.touch,
            footer: Rect::default(),
            pencil: Rect::default(),
        };
        dashboard.layout(env);
        dashboard
    }

    /// Places the page's widgets between the margins, top to bottom, as
    /// far as they fit above the footer; free spaces share the room the
    /// others leave.
    fn layout(&mut self, env: &Env<'_>) {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let pages = pages::pages(env);
        self.pages = pages.len();
        self.page = self.page.min(self.pages - 1);
        let margin = style.margin as u16;
        let gap = style.gap as u16;
        let width = display.width.saturating_sub(2 * margin);
        let footer_height = style.row as u16;
        let footer = Rect::new(
            0,
            display.height - footer_height,
            display.width,
            footer_height,
        );
        // The heights first: free spaces (`None`) take what is left.
        let column: Vec<(&String, Option<u16>)> = pages[self.page]
            .iter()
            .filter_map(|id| {
                if id == widgets::FILL {
                    return Some((id, None));
                }
                let height = env.widgets.height(id, env, width)?;
                (height > 0).then_some((id, Some(height)))
            })
            .collect();
        let used: u32 = column
            .iter()
            .filter_map(|(_, h)| *h)
            .map(|h| u32::from(h + gap))
            .sum::<u32>()
            + u32::from(gap);
        let fills = column.iter().filter(|(_, h)| h.is_none()).count();
        let fill = widgets::fill_height(u32::from(footer.y), used, fills);
        let mut y = gap;
        self.widgets.clear();
        self.targets.clear();
        for (id, height) in column {
            let Some(height) = height else {
                y += fill;
                continue;
            };
            if y + height > footer.y {
                break;
            }
            let area = Rect::new(margin, y, width, height);
            for target in env.widgets.targets(id, env, area) {
                self.targets.push((target.area, target.command));
            }
            self.widgets.push(Placed {
                id: id.clone(),
                area,
                shown: env.widgets.signature(id, env),
            });
            y += height + gap;
        }
        // The pencil, at the right end of the footer.
        let side = footer_height;
        self.pencil = Rect::new(display.width - margin - side, footer.y, side, side);
        self.targets.push((
            self.pencil,
            Command::OpenScreen(
                EDIT_SCREEN,
                Params::new().with(edit::PAGE, format!("{}", self.page)),
            ),
        ));
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

    fn run(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if let Some((_, command)) = self.targets.get(index) {
            cx.command(command.clone());
        }
    }

    fn edit(&self, cx: &mut Cx<'_, '_>) {
        cx.command(Command::OpenScreen(
            EDIT_SCREEN,
            Params::new().with(edit::PAGE, format!("{}", self.page)),
        ));
    }

    fn target_at(&self, point: Point) -> Option<usize> {
        self.targets.iter().position(|(r, _)| r.contains(point))
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
        let w = ui::icon_width(env, "pencil", size);
        let x = f32::from(self.pencil.x) + (f32::from(self.pencil.width) - w) / 2.0;
        ui::draw_icon(canvas, env, "pencil", size, x, self.pencil, Color::gray(60));
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

impl Screen for Dashboard {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        for placed in &self.widgets {
            ui::fill(canvas, placed.area, Color::WHITE);
            env.widgets.draw(&placed.id, canvas, env, placed.area);
        }
        self.draw_footer(canvas, env);
        // The focus: a frame around its target, once buttons are used.
        if self.focus_shown
            && let Some((area, _)) = self.targets.get(self.focus)
        {
            let frame = grown(*area, self.style.line * 2);
            canvas.stroke_rect(
                i32::from(frame.x),
                i32::from(frame.y),
                u32::from(frame.width),
                u32::from(frame.height),
                self.style.line * 3,
                Color::BLACK,
            );
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
        for placed in &mut self.widgets {
            let now = env.widgets.signature(&placed.id, &env);
            if now.is_some() && now != placed.shown {
                placed.shown = now;
                cx.request_redraw_area(placed.area);
                changed = true;
            }
        }
        let missing = blocks::wanted_covers(&env)
            .into_iter()
            .find(|path| covers::get(env.storage, path).is_none());
        if let Some(path) = missing {
            if covers::look_up(&env, &path).is_some() {
                for placed in &self.widgets {
                    if placed.id == pages::CONTINUE || placed.id == pages::RECENT {
                        cx.request_redraw_area(placed.area);
                        changed = true;
                    }
                }
            }
            cx.wake_at(env.now.add_millis(20));
        } else if self.widgets.iter().any(|p| p.shown.is_some()) {
            let next = quireink_sdk::time::next_minute(&env).unwrap_or(env.now.add_millis(60_000));
            cx.wake_at(next);
        }
        if changed {
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::SETTING_CHANGED || *event == events::STATUS_CHANGED {
            let env = *cx.env();
            self.layout(&env);
            cx.request_redraw();
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
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
            GestureKind::LongPress => {
                self.edit(cx);
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
